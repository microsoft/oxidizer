# TODO

## General

- Assess whether it would be possible to support serde-based deserialization into an arena

- Consider introducing arena-friendly hash map and hash set

## Non-idiomatic / surprising bits

- alloc<T: Send> requires Send. A bare arena.alloc(value) -> &mut T demanding T: Send will surprise people —single-threaded arena usage feels like it shouldn't need it. The reason (chunks/arena are Send and migrate threads)is real but subtle; this deserves a prominent doc note, and ideally a !Send-friendly path for thread-local-only use.
- Drain/Splice are eager, not lazy like std (now documented). Anyone relying on std'sdrop-timing/leak-amplification semantics will be caught out.

## Gaps / things genuinely missing

- Add missing try_xxx for any allocating functions.
- No owning IntoIterator for Box<[T]> (std has it). Minor, but an easy ergonomic win.
- Cross-type comparisons (Arc<[T]> == &[T], String == &str) — String has some via PartialEq<&str>, but Arc/Box only do Self == Self. std implements many cross-type PartialEqs.

## Zero-copy `Vec`/`String` → `Arc<[T]>` / `Arc<str>`

### Problem

`Vec` and `String` are backed by a `LocalChunk` (non-atomic refcount —
`LocalChunk::inc_ref` is `unreachable!`). Freezing them into `Arc<[T]>` /
`Arc<str>` therefore copies the data into a `SharedChunk` and returns an `Arc`
pointing at the copy. The copy is mandatory today because:

- `Arc` holds an **atomic** refcount on a `SharedChunk`, and
- a thin `Arc<[T]>` recovers its length from a `usize` prefix word and its chunk
  header via the 64 KiB `CHUNK_BASE_MASK`.

Bytes living in a local chunk satisfy neither, so the only way to avoid the copy
is to **build the buffer in a shared chunk from the start**. Customers building
data specifically to hand out as `Arc<[T]>` / `Arc<str>` want to skip that copy.

### Implementation crux (shared by all API options)

A *shared* growable buffer needs two things a local one doesn't:

1. **Hold a refcount on its backing shared chunk during growth.** Otherwise an
   interleaved `alloc_arc` that triggers `refill_shared` rotates the chunk out;
   with nothing holding it, the chunk can be torn down and the builder dangles.
   So a shared builder carries a `ChunkRef` (a `+1`), re-acquired on each
   relocation. (Note the interaction with the pre-credited surplus /
   `local_shared_count` accounting in `arena/mod.rs`.)
2. **Reserve the `Arc` length-prefix slot** up front so freeze is a pointer
   fix-up rather than a copy.

With those in place, `into_arc` becomes **O(1)**: write the final length into the
prefix slot, then `mem::forget` the builder's `ChunkRef` to transfer its `+1` to
the new `Arc`. Once this machinery exists, the API-shape choice below is
secondary and cheap to swap.

### API options

- **A — separate `SharedVec` / `SharedString` types.** Clearest intent,
  type-safe freeze, no runtime branch. Source duplication can be contained with
  a `impl_arena_vec_common!` macro emitting both flavors from one body (mirrors
  the existing `impl_arena_string_common!`).
- **B — one type, flavor chosen at construction via a runtime flag**
  (`alloc_vec` vs `alloc_shared_vec`). Smallest API, but a branch in the
  growth/freeze paths and no compile-time signal that `into_arc` is free vs. a
  copy.
- **C — one type with a zero-cost marker type parameter (recommended).**
  `Vec<'a, T, A = Global, F = Local>` with a sealed `ChunkFlavor` trait
  abstracting reserve / refill / oversized / ref-holding. `Local` = today's
  behavior (default, so existing code is unchanged); `Shared` holds the
  `ChunkRef` + prefix slot. Expose `SharedVec` / `SharedString` as type aliases.
  - common ops (`push`, `extend`, `len`, …) in `impl<F: ChunkFlavor>` — one body;
  - `into_arc` is O(1) only on `F = Shared`, still available (O(n) copy) on `Local`;
  - `into_slice` (arena-lifetime, no copy) stays `Local`-only.
  Zero runtime branch (monomorphized), type-safe freeze, single impl. Cost:
  generic noise in signatures, mitigated by the `F = Local` default and aliases.

`String` wraps `Vec<u8>`, so flavor support added to `Vec` extends to `String`
(and `Utf16String`) for free.

### Recommendation

Option C for zero-cost + compile-time freeze guarantees; fall back to A-via-macro
if the generic parameter is too noisy. All options require the same underlying
"shared growable buffer holds a `ChunkRef` + prefix slot" work — start there.

