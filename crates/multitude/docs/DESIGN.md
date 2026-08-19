# Multitude Architecture Overview

This document describes how `multitude` is put together and *why* it
behaves the way it does. It is a conceptual companion to the public-API
rustdoc (which documents *what* each method does) and to
[`PERF.md`](PERF.md) (which reports measured cost). The focus here is on
the runtime model: the moving parts, how they interact, the invariants
that keep them sound, and the edge cases and failure modes that fall out
of the design.

## Table of contents

- [The problem being solved](#the-problem-being-solved)
- [The four allocation styles](#the-four-allocation-styles)
- [Rust pinning model](#rust-pinning-model)
- [Architecture at a glance](#architecture-at-a-glance)
- [Chunk lifecycle](#chunk-lifecycle)
- [Reference counting without hot-path atomics](#reference-counting-without-hot-path-atomics)
- [Thin smart pointers: the alignment/masking trick](#thin-smart-pointers-the-alignmentmasking-trick)
- [Growable collections and zero-copy freeze](#growable-collections-and-zero-copy-freeze)
- [Arena-aware deserialization](#arena-aware-deserialization)
- [Concurrency model](#concurrency-model)
- [Configuration and tuning](#configuration-and-tuning)
- [Failure modes and edge cases](#failure-modes-and-edge-cases)
- [Safety invariants](#safety-invariants)
- [Risk areas and novel aspects](#risk-areas-and-novel-aspects)
- [Testing and verification](#testing-and-verification)

## The problem being solved

`multitude` is a bump allocator for **phase-oriented workloads** —
request handlers, parsers, compiler passes — where many allocations are
born together, live together, and die together. The win comes from two
places: allocation is a cursor bump (near free), and reclamation is a
single bulk operation instead of *N* individual frees.

The classic downside of a bump allocator is that it can *only* reclaim in
bulk: no individual object can be freed early, and nothing it hands out
may outlive the arena. `multitude`'s central design goal is to relax both
of those limits without taxing the common fast path:

- Individual chunks can be **reclaimed early** — as soon as the last
  handle referencing them drops — instead of waiting for arena teardown.
- Some handles (the smart pointers) can **outlive the arena** entirely,
  keeping just their backing chunk alive.
- Every allocated value has its **destructor run** at the right moment,
  automatically.

Everything below is machinery in service of those three properties.

## The four allocation styles

The defining architectural choice is that a single arena, and even a
single chunk, simultaneously supports four ways of owning a value. They
all bump-allocate from the same storage, all deref to the value, and all
run `T::drop` **eagerly** when ownership ends. They differ only in
ownership, escape capability, and what bookkeeping they pay:

| Handle | Ownership | Can outlive arena | Shared strong count | Cross-thread |
|---|---|---|---|---|
| `Alloc<'a, T>` | unique, `&mut` | no (bound to `&arena`) | none | move only |
| `Box<T>` | unique, `&mut` | yes | none | move only |
| `Rc<T>` | shared (`Clone`) | yes | non-atomic `u32` | no (`!Send`) |
| `Arc<T>` | shared (`Clone`) | yes | atomic `u32` | yes (`T: Send+Sync`) |

The key distinction that drives the whole chunk-lifecycle design is
**whether an owner family retains its chunk independently**:

- `Arc`/`Rc`/`Box` each take exactly **one** refcount on their hosting
  chunk at allocation. Because the chunk is kept alive by that count, the
  handle may escape the arena, and the chunk may reclaim early once its
  last handle drops.
- `Alloc<T>` carries **no** refcount. Its lifetime is tied to `&Arena` by
  the borrow checker, and the arena is responsible for keeping the
  backing storage alive for that borrow. This is the cheapest style —
  pure bump, no atomic, no prefix — at the cost of not being able to
  escape.

This split is why there is **no deferred-drop list**: because every
handle finalizes its own value when it drops, a chunk never has to
remember which slots still need a destructor at reset. Reset becomes a
pure cursor rewind.

## Rust pinning model

The crate's Rust `Pin` support follows the ownership of the allocation,
not merely the fact that arena storage happens to remain at a stable
address. A sound pinned owner must prevent the allocation from being
reused for as long as the pinning guarantee can matter, including when
the owner is deliberately forgotten.

That requirement produces three distinct policies:

- **Arena-bound `Alloc` is not pinnable.** Forgetting an `Alloc` ends its
  borrow without retaining the chunk independently. A later reset could
  then reuse the storage, so address stability during the ordinary handle
  lifetime is not enough to uphold `Pin`'s stronger contract.
- **A unique `Box` can be converted into a pin.** It independently retains
  its chunk, and forgetting it leaks that ownership rather than making the
  allocation reusable. This mirrors the standard unique-box model.
- **Shared `Arc` and `Rc` values can be pinned only at construction.** A
  fresh pinned constructor establishes the guarantee before any ordinary
  owner can escape. An existing shared owner cannot be converted later:
  an ordinary alias could survive the conversion, eventually become
  unique, and obtain mutable access to move a `!Unpin` value.

The ordinary shared owners provide uniqueness-checked mutable access when
their strong count is one. This remains compatible with pinning because a
pinned shared allocation never exposes an ordinary owner. Cloning through
the pinned abstraction preserves that separation and does not expose a
route back to unpinned ownership.

Shared uninitialized owners deliberately have no operation that combines
pinning with later initialization. `MaybeUninit<T>` is itself movable, so
such a surface could allow an ordinary shared owner to escape before the
initialized `T` became pinned. Callers instead construct the complete
value through a fresh pinned constructor.

Closure-based constructors do not provide emplacement. The closure
produces an ordinary value, which is then moved into its final allocation
before the owner is pinned. It therefore cannot create references to the
eventual allocation while the closure is running.

This use of Rust `Pin` is separate from the document's term **pinned
chunk**. A pinned chunk is one retained by the arena until reset because
it served arena-bound references; it says nothing about whether values in
that chunk are wrapped in `core::pin::Pin`.

## Architecture at a glance

Three collaborating types do the work, wired together by one deliberately
constrained chunk layout:

```text
                 ┌────────────────────────┐
                 │     ChunkProvider      │   factory + MPSC chunk cache
                 │  (one per Arena, Arc)  │   (Treiber freelist + size floor)
                 └───────────┬────────────┘
                     ▲       │ hands out fresh/cached chunks
              Weak   │       │
        (back-ref)   │       ▼
                 ┌───┴────────────────────┐
                 │        Arena           │   Send when A: Send + Sync; !Sync
                 │  ┌──────────────────┐  │
                 │  │  current chunk   │──┼──► bump-allocated here (hot path)
                 │  │  (ChunkMutator)  │  │
                 │  └──────────────────┘  │
                 │  retired-local list ───┼──► intrusive list of pinned chunks
                 └────────────────────────┘
                            │ allocations produce
        ┌───────────────────┼────────────────────┐
        ▼                   ▼                     ▼
   Alloc<'a,T>         Arc/Rc/Box            Vec/String
   (no refcount,       (one chunk            (grows in place;
    arena-bound)        refcount each)        freezes into Box/Arc)
```

- **`Arena`** is a thin façade: it owns one *current* chunk (via a
  `ChunkMutator` that holds the bump cursor), an intrusive list of
  *retired* pinned chunks, and a strong reference to its provider. It is
  the only public handle to the allocator.
- **`ChunkProvider`** is the factory and cache for chunks. Each arena owns
  exactly one; it is never shared between arenas. Chunks hold a `Weak`
  back-reference to it so an escaped smart pointer can return its chunk to
  the cache (or free it) even after the arena is gone.
- **`Chunk`** is a DST with an `[UnsafeCell<u8>]` payload tail. It holds
  a shared handle to the backing allocator, a refcount, and one intrusive `next`
  link (reused for either the retired list or the cache freelist, since a
  chunk is never on both at once). It does **not** carry the bump cursor —
  that lives transiently in whichever `ChunkMutator` owns it. Keeping the
  allocator behind a shared handle also keeps the header size independent of
  `A`, which is required by first-tile smart-pointer recovery.

The payload is `[UnsafeCell<u8>]` rather than `[u8]` for two reasons:
interior mutability lets multiple derived writers mutate through a shared
`&Chunk`, and carrying the whole payload as a fat pointer preserves
pointer provenance across the entire allocation region under Stacked/Tree
Borrows.

## Chunk lifecycle

A chunk moves through a small state machine. The transition that matters
most — what happens when the current chunk fills or the arena resets —
depends entirely on whether the chunk ever handed out an arena-lifetime
(`Alloc`) handle.

```text
        acquire (fresh or from cache)
                  │
                  ▼
            ┌───────────┐
            │  CURRENT  │
            │ (mutating)│
            └───────────┘
```

**Refill** (the chunk fills up) always rotates the chunk out of `CURRENT`:

```text
            ┌───────────┐  fills up
            │  CURRENT  │────────────► reconcile surplus, then:
            └───────────┘                          │
                    ┌──────────────────────────────┴────────┐
        handed out an Alloc?                         smart-pointer-only?
                    │ yes                                    │ no
                    ▼                                        ▼
            ┌───────────────┐                        last handle live?
            │  PINNED       │                       ┌────────┴────────┐
            │ (retired list,│                    yes│                 │no
            │  freed at     │                       ▼                 ▼
            │  reset/drop)  │                 stays alive         reclaim now
            └───────┬───────┘                 until handle       (→ cache or free)
                    │ reset / arena drop        drops
                    ▼                             │
              → cache or free  ◄──────────────────┘
```

**Reset** releases every older retired chunk, then keeps or detaches the
current one depending on whether a smart owner escaped from it:

```text
            ┌───────────┐  reset
            │  CURRENT  │────────────► no smart owner escaped?
            └───────────┘                          │
                    ┌──────────────────────────────┴────────┐
                    │ yes                                   │ no
                    ▼                                       ▼
            ┌────────────────────┐          reconcile surplus and detach;
            │  RETAINED          │          the chunk lives on until the
            │ (stays CURRENT,    │          last escaped handle drops
            │  cursor rewound,   │                      │
            │  marker cleared)   │                      ▼
            └─────────┬──────────┘                → cache or free
                      │                                 │
                      ▼                                 ▼
        serves the next generation           next alloc acquires a
                                             fresh CURRENT chunk
```

**Pinned chunks.** If a chunk handed out any `Alloc` handle (including the
`str`/`[T]`/`MaybeUninit` forms and growable-collection buffers), it must
stay alive until reset: those handles hold no refcount and are bounded
only by the `&Arena` borrow, so freeing the chunk could dangle a live
reference. Such chunks are threaded onto an intrusive singly linked
*retired list* and released in bulk at `reset`/`drop`.

**Early-reclaim chunks.** A chunk that handed out *only* smart pointers is
dropped from the current slot immediately on refill. It stays alive purely
through its handles' refcounts, so it reclaims the moment its last
`Arc`/`Rc`/`Box` drops — possibly long before the arena is reset, possibly
after the arena is gone.

**Mixed chunks pay the pin.** A chunk that served *both* an `Alloc` and a
smart pointer is pinned until reset even after its `Arc`s drop. This is the
deliberate, acknowledged cost of letting one current chunk serve both
styles; the arena tracks a single "did this chunk hand out a reference?"
flag so that only genuinely mixed chunks pay it. Local reservations sample
that flag after bounds checks but before committing the bump cursor, and write
it only for the false-to-true transition. This keeps the steady-state path free
of repeated marker stores and avoids a marker load after the cursor store.
The marker transition and chunk-exhaustion edges are laid out as cold blocks,
leaving the marked, in-capacity path as straight-line fall-through code.

**Reset is a cursor rewind.** `Arena::reset` takes `&mut self`, which
statically guarantees no `Alloc` (which borrows `&self`) is live. It runs
**no** destructors — every `Alloc` already ran its own on drop, and
smart-pointer values remain owned by their still-live handles. A current chunk
that handed out no smart pointer is retained with its pre-credited refcount and
rewound in place. Otherwise reset reconciles the current chunk's refcount
surplus (below) and detaches it, returning its bytes to the cache when the last
escaped handle releases it. Reset similarly releases only the list-owned
reference to each older retired chunk; an outstanding smart owner keeps that
chunk alive until the owner releases it.

The marker is cleared even when reset retains the current chunk: the next
generation may use that chunk exclusively for smart pointers and must still
qualify for early reclamation. Leaving a stale mark would silently pin it.
Encoding an additional "rewound but previously local" state would remove a
small part of the first-allocation boundary, but the measured path already
executes fewer instructions than Bumpalo and the extra lifecycle state is not
justified by the remaining wall-clock delta in
[`PERF.md`](PERF.md#reset-plus-the-next-allocation).

`ArenaStats` keeps its lifetime counters and live gauges across this boundary,
while its explicitly named `*_since_reset` counters start a new generation
only after reset completes successfully. They count backing allocations,
allocated backing bytes, cache reuse, and collection relocations attributable
to the current generation. If allocator deallocation panics during reset, the
generation counters remain intact so the interrupted phase is not reported as
successfully closed.

**Oversized allocations bypass this entirely.** A request larger than the
configured `max_normal_alloc` gets a one-shot chunk sized exactly to it,
filled through a stack-local mutator, and *never installed as current* —
so small allocations keep flowing into the existing active chunk. An
oversized reference chunk is pinned; an oversized smart-pointer chunk
reclaims with its handle.

## Reference counting without hot-path atomics

Naively, every smart-pointer allocation would bump the chunk's atomic
refcount — an atomic on the hot path. `multitude` avoids this with a
**pre-credited surplus** scheme:

1. When a chunk becomes current, its atomic count is pre-credited with a
   large surplus (2³⁰).
2. Each smart-pointer handout increments a **non-atomic** per-arena
   counter instead of the atomic; `Alloc` handouts touch neither.
3. When the chunk is retired (refill, reset, or drop), the surplus is
   reconciled with a **single** atomic subtraction, leaving the chunk's
   count equal to exactly the number of handles that actually escaped.

The 2³⁰ surplus is far larger than any plausible number of concurrent
`Arc::drop`s on other threads, so it cannot underflow before
reconciliation.

There are then **two** independent counts in play:

- **The chunk refcount** (atomic) tracks how many *families* of handles
  keep the chunk alive. The entire `Arc`/`Rc`/`Box` family for one value
  takes exactly **one** chunk refcount at allocation and releases it when
  the last member drops.
- **The shared strong count** tracks clones of a single shared value.
  `Arc::clone`/`Rc::clone` bump *only* this count (a relaxed atomic
  increment for `Arc`, a plain non-atomic increment for `Rc`); they never
  touch the chunk count.

On the last-clone drop, the value's destructor runs in place
(`drop_in_place`, which natively handles `?Sized`), and *then* the
family's single chunk refcount is released. The chunk refcount is adopted
*before* the value drop runs, so even a panicking destructor still
releases the chunk.

Because destructors run eagerly on the last clone rather than being
deferred to chunk teardown, nested arena handles — e.g. `Arc<[Arc<T>]>`
whose inner and outer values share a chunk — release promptly instead of
forming a self-pinning cycle.

## Thin smart pointers: the alignment/masking trick

Every escape-capable smart pointer stores an 8-byte payload pointer on
64-bit. Handles for sized values, `str`, slices, and other
`usize`-metadata DSTs contain only that word. Trait-object handles add a
second word for the vtable, allowing differently coerced views of the
same shared allocation to coexist. This hybrid representation rests on
one geometric invariant:

> **Every chunk allocation is 64 KiB-aligned** (`CHUNK_ALIGN = 65 536`).

Given that, any value pointer can recover its owning chunk's header by
simply **masking off the low 16 bits** — no type tag, no back-pointer
stored per value. `Box::drop`, `Arc::drop`, and `Rc::drop` all recover
`*const Chunk` the same way.

DST metadata is stored according to its kind:

```text
metadata `()`      allocation: [T payload]          handle: [payload pointer]
metadata `usize`  allocation: [length][T payload]  handle: [payload pointer]
trait object      allocation: [T payload]          handle: [payload pointer][vtable]
```

The representation uses a defaulted, sealed metadata type parameter on each
smart pointer. That parameter is a real part of the public type signature even
though callers normally never name it. For sized and `usize`-metadata pointees
it is `()`, so the compiler gives it no storage and the handle stays one word.
For `DynMetadata`-backed pointees it is the vtable word. Hiding metadata in an
internal enum or raw-word field would make that field occupy space in every
smart-pointer monomorph, enlarging the common handles to at least two words.
The defaulted parameter is the deliberate API tradeoff that lets only
vtable-carrying handles pay the extra word and avoids a metadata tag branch.

Trait-object coercion is explicit because downstream implementations of
`core::ops::CoerceUnsized` and `core::marker::Unsize` require unstable Rust
features. `coerce!` instead generates a local function whose raw-pointer
conversion uses the compiler's built-in unsizing coercion, then packages that
function as a proof token for the stable API. `Coercion::new` remains an unsafe
low-level escape hatch for advanced callers; applying its token always checks
that the function preserved the data-pointer address before accepting the
resulting metadata.

`Arc` and `Rc` additionally place their family's shared strong count at
the start of the allocation reservation, before any padding and
allocation-resident metadata. `Box` has no strong count. Length and strong
count fields are recovered from the payload pointer and are read or
written with the alignment discipline appropriate to each field.

Consequences of the masking scheme, each a real edge case:

- **The smart-pointer alignment ceiling is 32 KiB** (`CHUNK_ALIGN / 2`). A
  request at or above that value cannot be guaranteed to lie inside the first 64 KiB
  tile, so it is rejected — `try_alloc_*` returns `AllocError`, `alloc_*`
  panics.
- **Oversized chunks** are still 64 KiB-aligned and place their single
  value at the payload start, inside the first tile, so the same mask works.
- **End-of-chunk ZST guard.** A zero-sized allocation landing exactly at
  `chunk_base + CHUNK_ALIGN` would mask to the *next* chunk. Two rules
  prevent it. Every reservation must fit `size.max(1)` bytes below the
  chunk limit, so a request whose payload would end exactly at the
  boundary is routed through refill instead of returning a one-past-end
  pointer. Independently, every smart-pointer reservation floors its
  payload at one byte, so each ZST handle gets its own distinct in-chunk
  address and a chunk can host only a bounded number of them.

The alignment is enforced at allocation time via the `Layout`, not via
`repr(align)` on the chunk struct — keeping the struct's structural
alignment small so `size_of_val` matches the real allocation even for the
smallest size classes.

`Rc` reuses all of this — compact handles for sized and slice-like
pointees, header masking, allocation-resident lengths, the family's
single chunk refcount, and eager last-drop teardown — with two
differences. Its strong count is a plain non-atomic `u32` (sound because
`Rc` is `!Send`/`!Sync`), and the reservation need not be 4-byte aligned.
For sub-4-aligned payloads (`str`, `[u8]`), this can avoid up to three
bytes of inter-allocation alignment gap; the strong-count field itself is
still four bytes. Because `Rc` imposes no `Send`/`Sync` bound on `T`, it
can own thread-affine values (e.g. `Rc<RefCell<T>>`) that `Arc` cannot.

## Growable collections and zero-copy freeze

`Vec<T>`, `String`, and `Utf16String` are **transient builders**:
small (40-byte on 64-bit targets) mutable handles over an arena buffer, meant to be built
up briefly and then *frozen* into an immutable smart pointer.

While live, a growable buffer pins its chunk through the same
reference machinery as `Alloc` (no chunk refcount), so the hot push/grow
path is a plain bump with no atomics. When it can't grow in place it
**relocates** to a larger buffer (counted by `ArenaStats::relocations`);
the abandoned buffer is dead space reclaimed at reset.

The headline feature is **zero-copy freeze**. Every freezable buffer
reserves the full shared-slice freeze prefix (`[strong][pad][len]`) in front
of its payload at allocation time. The bit pattern is suitable for
`Arc<[T]>` and `Rc<[T]>`; `Box<[T]>` uses the length and ignores the
strong field. Freezing into any of those owners then:

1. recovers the hosting chunk by the 64 KiB mask;
2. adopts the family's chunk refcount (from the pre-credited surplus if
   the buffer is still in the current chunk, otherwise a plain atomic bump);
3. writes the final length into the reserved slot; and
4. `mem::forget`s the builder, transferring element ownership to the smart
   pointer.

No allocation, no element copy. The freeze also returns unused tail
capacity to the arena when it can.

**When zero-copy doesn't apply**, freeze falls back to an O(*n*) element move:

- ZST buffers do not carry a real allocation prefix and freeze by moving
  their elements into an owning allocation.
- A zero-copy `split_off` tail whose base points mid-chunk has no prefix
  and moves its elements.
- Any other buffer without a freeze prefix uses the same move fallback.
- An over-aligned smart-pointer destination (`T` alignment ≥ 32 KiB) is
  unsupported: a fallible freeze reports `AllocError`, and an infallible
  freeze panics.
- `Vec::leak` → `&mut [T]` is O(1) and allocation-free when `T` does not
  need drop (reinterpret in place), but the result does **not** outlive
  the arena.

A one-bit `freeze_prefix` flag on each buffer records which path applies.
`String` and `Utf16String` wrap the same buffer machinery, so their normal
`Box`/`Rc`/`Arc` freezes are also in-place.

## Arena-aware deserialization

Deserialization extends the ownership model into Serde rather than replacing
Serde with a format-specific object mapper. The central abstraction is an
allocator-aware counterpart to ordinary `Deserialize`: every recursive step
receives the arena, so fields can choose arena-backed storage while preserving
Serde's streaming, borrowing, and data-model semantics.

```text
 encoded input
      │
      ▼
 format deserializer ──► optional resource-limit boundary
      │
      ▼
 arena-aware seed carrying &Arena
      │
      ├──► scalars and ordinary values
      ├──► arena-owned strings, slices, and smart pointers
      ├──► borrowed input where the format can expose it safely
      └──► explicitly delegated ordinary-Serde fields
```

This is deliberately **opt-in and structural**. A type derives or implements
arena-aware deserialization, and that decision is propagated through its
fields. There is no blanket fallback from every ordinary `Deserialize` type:
such a fallback would overlap arena-aware implementations and, more
importantly, could silently allocate through the global allocator. A field may
explicitly delegate to ordinary Serde when that allocation behavior is
intended.

### Storage and lifetime follow the target type

The target type determines where decoded data lives:

- Arena `Box`, `Arc`, and `Rc` fields own their decoded strings, slices, or
  values in arena chunks and have the same escape behavior as values allocated
  directly through the arena.
- An `Alloc` root is tied to the arena borrow. An escape-capable smart-pointer
  root independently retains its chunk, so a fully arena-owned graph can
  outlive the arena handle.
- Arena `Cow` borrows input only when the source deserializer can provide data
  valid for the input lifetime. Otherwise it stores a decoded copy in the
  arena. For JSON, an unescaped string can be borrowed, while an escaped string
  must be decoded and owned.
- Ordinary collections may contain arena-aware elements while retaining their
  own ordinary buffers or nodes. Frozen arena slices are the usual choice when
  the sequence storage itself must belong to the arena.

This separation makes mixed graphs explicit: arena ownership is not inferred
from the mere presence of an arena at the root.

### Derived, custom, and dynamic data

The derive supports the structural forms that can be decoded directly through
Serde's visitor model, including structs and externally tagged enums, while
honoring the corresponding naming, defaulting, unknown-field, and custom-field
rules. Representations that require hidden buffering or replay, such as
untagged or internally tagged enums and flattened fields, are rejected rather
than weakening the caller's input-borrowing contract.

Custom implementations use the same arena-carrying seed model, allowing
arena-aware values to participate inside larger Serde visitors without
introducing a separate data format.

For intentional buffering, the dynamic `Value` model captures arbitrary Serde
data in arena storage and can replay it through an ordinary Serde deserializer.
Its maps preserve insertion order, duplicate keys, and non-string keys. Replay
is limited by what the source format exposes: opaque enum-access protocols do
not always reveal enough structure for a format-independent capture.

### Limits and failure semantics

Optional deserialization limits bound nesting depth, sequence and map lengths,
string length, and byte-string length. They form a wrapper around the
format-independent seed path, so the same policy applies to generic
deserializers and JSON helpers. Reported size hints are clamped before
reserving storage; they are optimization hints, never trusted declarations of
the eventual input size.

Serde requires format-independent allocation and limit failures to use the
source deserializer's error type. Resource-limited JSON helpers add a typed
boundary around that channel: `JsonError::limit_exceeded` reports the resource
and configured limit, while malformed and incompatible input preserves the
underlying `serde_json::Error`. A failed operation is **not transactional**:
already consumed arena capacity remains consumed. General rollback would be
unsound because custom deserialization can create escape-capable owners before
a later field fails.

Reusable arena `String` and `Vec` builders offer a narrower replacement model.
They clear their logical contents while retaining capacity, then decode into
that existing buffer. On failure the builder remains valid but may contain the
successfully decoded prefix. Reuse applies across several refreshes within one
arena generation; reset invalidates the borrowed builder, after which a new one
can benefit from the arena's warm chunk cache.

Top-level JSON arrays can instead be consumed as a stream of independently
owned values. Each value is delivered in wire order and dropped after its
callback unless the callback moves selected arena-owned fields elsewhere. This
avoids allocating a root sequence buffer, but does not make decoding lazy:
every delivered value is fully deserialized first. Syntax, shape, allocation,
or limit failures may occur after an earlier prefix has already produced
observable callback effects. The fallible callback variant preserves the
callback's error and stops immediately; because it intentionally abandons the
deserializer, the unconsumed suffix is not validated. When every callback
succeeds, complete-input and trailing-data validation remain unchanged.

JSON support is a convenience layer over the same architecture. It accepts
string or byte input, requires exactly one complete JSON value, rejects trailing
non-whitespace data, and offers the same resource limits, vector-reuse, and
streaming semantics. Trailing-input rejection occurs after a streamed array has
been delivered. Decoding escaped JSON strings may require temporary parser
scratch space even when the final value is arena-owned.

## Concurrency model

`Arena<A>` is **`Send` when `A: Send + Sync`, but always `!Sync`**. The
whole arena — with its in-flight `Alloc` handles and smart pointers — can
move between threads when its backing allocator permits it, but it cannot
be *shared*; cross-thread sharing is done by allocating an `Arc` and cloning
it across threads. The `Sync` bound on `A` is required because chunks share
one allocator instance and may be reclaimed concurrently on other threads.
`Arena`'s own `!Sync` is structural (the current chunk, the per-arena cells,
and the retired list are all `!Sync`), and it is load bearing:

The chunk cache is a single intrusive **Treiber-style freelist**, but the
`!Sync` arena makes it **multi-producer / single-consumer**. *Pushes*
(returning a chunk to the cache) happen from any thread that drops the last
handle on a chunk; *pops* happen only from the arena's owning thread. MPSC
sidesteps Treiber's classic hazards for free: no rival consumer can free
the head between our load and CAS (no use-after-free), and the head's
identity can't recycle behind our back (no ABA). A monotonic size-class
*floor* rides alongside the freelist; when it advances, below-floor chunks
still on the list are walked and destroyed in one pass.

`Rc` opts out of all of this: being `!Send`/`!Sync`, its non-atomic count
never crosses a thread boundary, which is exactly what makes the
non-atomic count sound.

## Configuration and tuning

`ArenaBuilder` exposes the tuning knobs; all have defaults that reproduce
`Arena::new()` exactly.

- **`max_normal_alloc`** (default 16 KiB, range `[4096, chunk-max]`) — the
  payload-size threshold above which a request bypasses the cache and gets
  a one-shot oversized chunk. Out-of-range values panic at build with the
  resolved bounds.
- **`byte_budget`** (default unbounded) — a cap on total outstanding chunk
  bytes (live + cached), enforced by a CAS loop; hitting it surfaces as an
  allocator failure.
- **`with_capacity`** — preallocates chunk bytes up front and seeds the
  size-class ratchet, so a warm-up preallocation is consumed by the first
  refill rather than being re-grown from the smallest class.

Two adaptive behaviors run without configuration:

- **Size-class ratchet.** Cacheable chunks come in eight power-of-two
  total sizes (512 B … 64 KiB). Each successful refill bumps a per-arena
  "next class" hint toward the largest class, so a long-lived arena stops
  paying the "always smallest class" refill tax.
- **Chunk cache floor.** The provider only caches up to the current floor
  class and destroys stragglers below it, bounding cache footprint as the
  working set's typical chunk size grows.

With the `stats` feature, `Arena::stats` returns a low-cost snapshot. Lifetime
counters report backing allocations, cache reuse, resets, and buffer
relocations. Live gauges report bytes held, cached chunks and bytes, and wasted
tail bytes; the byte high-water mark survives reset and reclamation. Cached
bytes are a subset of total held bytes: active, retired, and independently
retained chunks remain outside the cache. Because escaped atomic owners can
return chunks from other threads, fields in one snapshot may describe adjacent
instants rather than one globally synchronized state.

## Failure modes and edge cases

**Allocation failure** is reported by `AllocError`, which distinguishes
three mutually exclusive kinds so callers can react appropriately:

| Kind | Meaning | Retryable? |
|---|---|---|
| allocator failure | backing allocator returned null, or `byte_budget` exhausted | maybe (free memory / raise budget) |
| alignment too large | requested alignment ≥ 32 KiB smart-pointer cap | never — request is inherently unsatisfiable |
| capacity overflow | layout arithmetic wrapped `usize` or exceeded `isize::MAX` | never |

Every allocation comes in two flavors: `try_alloc_*` returns
`Result<_, AllocError>`; `alloc_*` panics on the same conditions. Choose
`try_*` on paths that must degrade gracefully.

**Refcount overflow aborts.** The chunk refcount aborts the process if it
would overflow, and a shared strong count aborts once a clone would push it
past a saturation threshold of `u32::MAX >> 1` — before the count can wrap.
The abort is `std::process::abort`, or a forced double-panic under `no_std`.
This mirrors `std::sync::Arc`: a wraparound would race live pointers against
a free, and termination is the only sound response. The abort helper is
`#[cold]`/`#[inline(never)]` so the hot path stays small.

**Panic safety.** Smart-pointer construction takes a protective `+1` guard
on the chunk *before* invoking the user's initialization closure; on
unwind the guard's `Drop` releases the `+1`, and on success ownership of
the `+1` transfers into the finished pointer. For arena-lifetime
allocations, a closure that panics before initializing leaves the reserved
slot untouched and constructs no `Alloc`, so no destructor runs on
uninitialized memory; slice initializers additionally drop any
already-initialized prefix on unwind. The net guarantee: a panicking
closure leaks no refcount and queues no `drop` on uninitialized memory.

**`mem::forget` is always sound**, but skips finalization — forgetting an
`Alloc` never runs its value's destructor; forgetting a smart pointer
leaks its refcount (and thus pins its chunk).

**Escaped handles outliving the arena** is a supported, not exceptional,
case: a chunk holds a `Weak` back-reference to its provider and a shared
allocator handle, so when its last handle drops after the arena is gone it
either returns itself to a still-living cache or frees its own backing
memory directly.

## Safety invariants

The design rests on a handful of invariants; violating any one would be
unsound, so they are maintained centrally rather than at each call site:

- **64 KiB chunk alignment** — the sole basis for header recovery by
  masking. Every chunk allocation, normal or oversized, honors it.
- **Smart-pointer alignment < 32 KiB** — guarantees every value pointer
  lies strictly inside its chunk's first tile, so the mask never walks to
  a neighbor. Enforced at allocation.
- **In-chunk payload address** — no reservation returns the one-past-end
  boundary pointer, and every smart-pointer payload occupies at least one
  byte, protecting the ZST edge case.
- **Pin-if-referenced** — any chunk that handed out a refcount-free
  `Alloc` stays alive until `&mut self` reset, so an `Alloc`'s borrow can
  never dangle.
- **Rust pinning follows retained ownership** — `Alloc` is not pinnable;
  unique `Box` pinning retains its chunk independently; and shared pinning
  is established only during fresh construction, before an ordinary alias
  can escape.
- **Refcount before value drop** — a family's chunk refcount is adopted
  before the value's destructor runs, so a panicking destructor still
  releases the chunk.
- **Prefix counts accessed only as raw reads/writes**, never through a
  reference spanning possibly-uninitialized payload — which keeps the
  scheme sound under Miri.

## Risk areas and novel aspects

This section is for the reader about to change the code. It names what is
unusual about the design, which invariants are load bearing, and what
enforces each of them today. It does not restate the invariants themselves;
those live in [Safety invariants](#safety-invariants) and
[Failure modes and edge cases](#failure-modes-and-edge-cases).

### What is unusual compared with a textbook bump allocator

A classic bump allocator is a cursor, a limit, and a bulk reset. Everything
in the list below is a departure from that model, and each one has
consequences that are not obvious from reading a single call site.

| Aspect | Why it is unusual |
|---|---|
| Four ownership styles in one chunk | `Alloc`, `Box`, `Rc`, and `Arc` bump from the same cursor. The chunk, not the arena, is the unit of lifetime, and one chunk can simultaneously host refcount-free and refcounted handles. |
| Early chunk reclamation | A chunk that served only smart pointers is reclaimed when its last handle drops, possibly long before reset, possibly on another thread. A textbook arena reclaims nothing before teardown. |
| Handles outliving the allocator | `Box`/`Rc`/`Arc` remain valid after the `Arena` is dropped. The chunk carries its own allocator handle and a `Weak` back-reference to the provider so it can free or re-cache itself with no arena present. |
| Pre-credited refcount surplus | The per-allocation atomic is removed entirely, replaced by one atomic at chunk install and one at retire, with a non-atomic per-arena tally in between. |
| Alignment masking instead of back-pointers | The owning chunk header is recovered arithmetically from any value pointer. No value stores a pointer to its chunk, so no per-value bookkeeping word exists. |
| Defaulted sealed metadata parameter | Handles for sized and `usize`-metadata pointees stay one word; only vtable-carrying handles pay a second word, without a runtime tag or branch. |
| Zero-copy freeze | A growable builder becomes an immutable `Box`/`Rc`/`Arc` slice with no copy, because the shared-owner prefix was reserved before the first push. |
| Stable-Rust unsizing | `coerce!` synthesizes a proof token from a compiler-generated raw-pointer coercion, so trait-object handles work without `CoerceUnsized`/`Unsize`. |
| Arena-threading Serde | The arena is carried through every recursive deserialization step rather than being consulted once at the root. |

### Fragile invariants

Each row states the invariant, what breaks if a change violates it, and what
currently keeps it true. These are the places where a small, locally
reasonable edit has non-local consequences.

| Invariant | What breaks if violated | What enforces it |
|---|---|---|
| Chunks are allocated 64 KiB-aligned | Header recovery by masking reads a non-header address; every `Drop` becomes a wild write | The chunk allocation `Layout` (base alignment is raised to `CHUNK_ALIGN` independently of the struct's own alignment); unit tests over the computed layout; `tests/chunk_footprint.rs` pins the *requested* size to the size class so the alignment is not obtained by inflating every chunk to 64 KiB |
| Smart-pointer alignment stays below 32 KiB | An over-aligned payload can be pushed past the first 64 KiB tile, so its mask resolves to the *neighboring* chunk | The allocation path rejects requests at or above `CHUNK_ALIGN / 2` with an `AllocError`; `buffer_freezable` gates the freeze prefix on the same bound |
| Chunk header size is independent of the allocator type `A` | A large `A` embedded by value in the header would push the first payload out of the first tile | The chunk stores a shared handle to the allocator rather than an `A` by value; `tests/audit_repro.rs` allocates through a deliberately huge allocator type and checks payloads still resolve |
| Surplus reconcile is balanced and happens exactly once per install | An under-refund pins the chunk forever (a leak); an over-refund frees a chunk that still has live handles (use-after-free) | The install and retire paths are the single writer of `local_shared_count`; every retire route — refill, `reset`, arena drop — funnels through the same `refund = SURPLUS - local` computation; loom models a worker `Arc::drop` racing the owner's reconcile |
| The chunk refcount is adopted before the value destructor runs | A panicking `T::drop` unwinds past the release and leaks the chunk permanently | The drop paths bind the recovered `ChunkRef` before calling `drop_in_place`, so the release happens in the guard's own `Drop`; `tests/unwind_safe.rs` and `tests/drop_reentrancy.rs` exercise the unwinding and re-entrant cases |
| Prefix counts are touched only through raw reads/writes | Forming a reference that spans uninitialized payload is UB even if never read, and is reported by Miri | The strong-count and length accessors take raw pointers and use (un)aligned raw loads/stores chosen per policy; `Rc`'s non-atomic count is deliberately accessed unaligned so its reservation need not carry `STRONG_ALIGN` |
| The arena is `!Sync` | The chunk cache stops being single-consumer, reintroducing Treiber-stack ABA and use-after-free between the head load and the CAS | `Arena` is structurally `!Sync` (interior cells, current chunk, retired list); `tests/send_sync.rs` locks the auto-trait contract so a new field cannot silently change it |
| A chunk that handed out an `Alloc` is never reclaimed early | A refcount-free `&mut T` borrow dangles while the borrow checker still believes it is live | The `current_has_reference` flag is set by every arena-lifetime reservation path and consulted on rotation; `reset` takes `&mut self` so no `Alloc` can be live across it |
| No reservation returns a one-past-end pointer | The masked header resolves to the following chunk | The bump path requires `size.max(1)` bytes of headroom below the limit; smart-pointer reservations additionally floor the payload at one byte |

Two further hazards are behavioral rather than structural:

- **Mixed chunks pay a pinning cost.** A chunk that served both an `Alloc`
  and a smart pointer stays alive until reset even after every smart pointer
  is gone. A change that widens which paths call the reference-handout marker
  silently converts early-reclaim chunks into pinned ones: no test fails, but
  steady-state footprint grows.
- **Deserialization is not transactional.** A failure part-way through leaves
  consumed arena capacity consumed and may leave escape-capable owners already
  constructed. Adding a rollback would be unsound for exactly that reason, so
  error paths must be written to tolerate partial progress rather than assume
  it can be undone.

### Sharp edges for users

- Values with alignment at or above 32 KiB cannot be placed in a
  smart pointer; the fallible APIs report `AllocError` and the infallible
  ones panic. Arena-lifetime allocations are not subject to the same cap.
- A chunk that served any arena-lifetime handle stays resident until
  `reset`, even if every value in it is already dropped.
- `mem::forget` on a smart pointer leaks its chunk refcount and therefore
  keeps the whole chunk resident for the life of the process.
- Refcount exhaustion terminates the process rather than returning an error.
- Deserialization failures consume arena capacity and are not rolled back.
- `Vec::leak` is allocation-free only for types that need no drop, and its
  result still does not outlive the arena.

## Testing and verification

The crate's correctness argument does not rest on one test suite. Each of
the properties the design depends on — memory-model correctness, provenance
discipline, unwind safety, cross-thread reclamation, and the absence of
global-allocator traffic — is falsifiable by a different tool, so the
verification layers are chosen to match the failure modes rather than to
maximize coverage percentages.

| Layer | What it can falsify that nothing else can |
|---|---|
| Integration and unit tests | Functional behavior and the API contracts that are statically checkable (auto traits, variance, unwind-safety bounds) |
| Miri, base profile | Out-of-bounds access, invalid aliasing, uninitialized reads, and misaligned access across the whole unsafe surface |
| Miri, `-Zmiri-tree-borrows` | Aliasing violations that Stacked Borrows accepts, which matters because payload writes go through shared `&Chunk` borrows |
| Miri, `-Zmiri-strict-provenance` | Provenance loss — directly relevant because chunk recovery is address arithmetic on a value pointer |
| Miri, `-Zmiri-many-seeds` | Data races and ordering bugs whose manifestation depends on the scheduler seed |
| `cargo careful` | UB that a hardened `std` catches at runtime, on real threads that Miri does not execute |
| Loom | Memory-ordering bugs in the refcount and cache protocols, by exhaustively permuting legal interleavings |
| Bolero | Lifecycle invariant violations reachable only through long, unplanned operation sequences |
| `cargo mutants` | Assertions that never actually constrain behavior — a test suite that passes with the logic removed |
| `alloc_tracker` | Silent regressions to global-allocator traffic, which no functional test would notice |

**Miri** runs over the library and integration tests with all features in
CI, and the tree-borrows, strict-provenance, and many-seeds profiles run on
a nightly schedule. A repo-root suppression file, `.miri-tree-borrows-skip`,
excludes individual tests from the tree-borrows profile only. Entries there
are memory-budget exclusions, not soundness exclusions: tree borrows tracks
provenance per byte, and a test that takes repeated nested borrows into a
64 KiB chunk can exceed the CI runner's memory limit while remaining
correct. Two test binaries opt out of Miri entirely for structural
reasons — the Bolero driver needs filesystem access for corpus replay, and
the allocation-tracking tests measure real system-allocator traffic that
Miri's allocator model does not represent. Both have Miri-visible
counterparts covering the same unsafe paths.

**Loom** model-checks the concurrent protocols. The test target is gated
behind a marker feature, built under `--cfg loom`, and driven by
`just loom`. It covers five protocol families:

| Interleaving explored | Invariant defended |
|---|---|
| Concurrent `Arc::clone`/`drop` on the same allocation, including a sized handle racing a trait-object view of it | The value's destructor runs exactly once no matter which handle observes the last decrement, and coerced handles participate in the same family count |
| The owner's surplus reconcile racing a worker's `Arc::drop` | The single `fetch_sub(SURPLUS - local)` and concurrent per-handle decrements settle to exactly zero, with teardown running once |
| Handle drops racing `Arena::reset` and `Arena` drop, including allocation from a chunk shared with a prior generation | A chunk retained by an escaped handle survives reset, and reclamation is neither doubled nor skipped when the owner tears down first |
| Two threads pushing chunks onto the cache, and a push racing the owner's pop | The Treiber stack's CAS retry loops are correct, and the popper's read of the head's `next` is sound against a concurrent push — the property that MPSC discipline is supposed to buy |
| Cross-thread `assume_init` on clones of one uninitialized allocation | The release/acquire chain that publishes the drop shim before chunk teardown reads it |

**Bolero** drives generated sequences over an operation alphabet spanning
every allocation style, freeze path, and `reset`, then asserts that the
number of constructed payloads equals the number dropped once the arena is
gone. The payload types are chosen to hit the awkward paths: a ZST, a
256-byte-aligned type, and a payload larger than `max_normal_alloc` that
forces the oversized-chunk route. Dedicated variants force LIFO drop
ordering and interleave a `reset` between every operation, which is the
cheapest way to reach the eviction-and-cache-pop orderings.

**Mutation testing** runs diff-scoped on pull requests and in full on a
nightly schedule. Paths where a mutant would turn a fallible reservation
into an infinite refill spin carry explicit `mutants::skip` annotations with
the reason recorded at the site, so surviving mutants indicate real gaps
rather than known-unkillable ones.

**Allocation tracking** pins the property the whole design exists to
deliver. Using a counting global allocator, it warms an arena until the
size-class ratchet and the chunk cache reach steady state, then repeats the
identical workload and asserts that the number of bytes requested from the
system allocator is exactly zero — across both `reset` and the
all-handles-dropped reclamation path. A regression that quietly stops
reusing cached chunks would still be functionally correct and would still
pass every other layer.

**Integration tests** are grouped by the failure class they defend, not by
API surface:

| Class | What it protects |
|---|---|
| Allocation styles | That each of `Alloc`, `Box`, `Rc`, `Arc`, `Vec`, `String`, and `Utf16String` honors its own ownership, escape, and drop-timing contract |
| Unwind safety | That a panic in a user closure, a `Clone`, or a destructor leaks no refcount and never queues a drop on uninitialized memory, including the growable-collection resize guard, which is isolated in its own binary so its process-global panic hook cannot perturb other tests |
| Drop re-entrancy | That a destructor which itself drops arena handles — including the last handle on another chunk, and including teardown re-entering allocation — cannot corrupt the chunk being torn down |
| ZST edge cases | That zero-sized payloads receive distinct in-chunk addresses, initialize and drop exactly once, and never produce a boundary pointer that masks into the next chunk |
| DST and unsizing | That trait-object coercion preserves the data-pointer address, that hybrid one-word and two-word handle layouts interoperate, and that coercion of built-in trait objects such as `dyn Any` works without the optional DST feature |
| Variance and auto traits | Compile-time assertions that the handles are covariant in `T` and that `Send`/`Sync`/`UnwindSafe` follow ownership, so a new field cannot silently widen a bound |
| Layout and footprint | That the bytes requested from the backing allocator match the intended size class rather than being inflated to the chunk alignment, and that a large allocator type does not displace payloads out of the first tile |
| Standard-library parity | That the `std`-shaped surface on the growable collections — `From`-based freezing, `leak`, `split_off`, `spare_capacity_mut`, indexing, and the conversion traits — behaves as the equivalent `std` API does |
| Cache behavior | That the size-class floor ratchets monotonically and that below-floor chunks are evicted or destroyed rather than accumulating |
| Third-party integration | That the `bytemuck`, `zerocopy`, `bytes`, `bytesbuf`, and `hashbrown` bridges uphold their own crates' contracts over arena storage, including `hashbrown` table growth |
| Serialization | Arena-aware deserialization round-trips, borrow-versus-copy decisions, limit enforcement, and the non-transactional failure semantics |
| Pinning | That the three pinning policies hold for scalar, uninitialized, slice, and DST allocations |

**Benchmarks** pair Criterion wall-clock suites with Callgrind
instruction-count suites over shared measured bodies, so a change can be
attributed to instruction count rather than host noise. A curated
wall-clock subset is published in [`PERF.md`](PERF.md).
