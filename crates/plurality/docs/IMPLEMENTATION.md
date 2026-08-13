# Plurality — Implementation

This document set describes how the architecture in [`DESIGN.md`](./DESIGN.md)
is realised internally. It covers the parts that are not user-visible: how the
pool forms share one body of machinery, how slot geometry is computed and kept
consistent, how handles find their way home without a back-pointer, how the
layout router works, and how the whole thing is measured and verified. For the
public contract see the crate-level rustdoc; for ideas that are not part of the
crate, see [`TODO.md`](./TODO.md).

This page is the orientation layer: the module map, the layering, and the
overall shape of the machinery. Each area document takes one part of that
picture and goes deeper.

## Area documents

- [Slot geometry](./implementation/geometry.md) — the formulas that place a
  value, its reference count and its index inside a slot, the geometry provider
  that supplies them at compile time or at run time, and the checks that keep
  the two derivations honest.
- [The pool body](./implementation/pool-body.md) — the core data structures,
  the slot lifecycle and the intrusive free list, the chunk directory and its
  index arithmetic, growth, pointer recovery, the two reference counts,
  teardown, construction, failure and statistics.
- [Handles](./implementation/handles.md) — the four owning handle flavors:
  their representation, drop paths, auto-trait and variance behaviour, the
  shared macro-generated surface, coercion and pinning.
- [The multi pool](./implementation/multi-pool.md) — the crate-private layout
  pool and the router in front of it: its state, lookup, interior mutability,
  installation, and ownership of the layout pools.
- [Allocator reentrancy](./implementation/reentrancy.md) — how allocator calls,
  multi-pool allocator cloning, and user-code callbacks can re-enter pools, and
  how cold paths keep state consistent.
- [Performance](./implementation/performance.md) — the cost model, what must
  not regress, and the benchmark decomposition that attributes each cost.
- [Verification](./implementation/verification.md) — the layered test strategy
  and the tools that implement it.

## Module map

Every module is private; the public surface is a set of re-exports from the
crate root plus the exported `coerce!` macro. The crate is `no_std` and depends
on `alloc`. The allocator abstraction comes from a single runtime dependency
and appears in the public API, so its two types are declared as permitted
external types.

| Module | Contents |
|---|---|
| `pool` | `Pool`, `PoolCore`, `PoolInner`, the slot lifecycle, growth, pointer recovery and teardown. |
| `multi_pool` | `MultiPool`, the layout router. |
| `layout_pool` | `LayoutPool`, the crate-private pool keyed on a runtime layout. |
| `geometry` | The slot-geometry abstraction, its compile-time and run-time providers, and the two directions of slot addressing. |
| `chunk` | `ChunkHeader` and the typed chunk-layout helper. |
| `directory` | `reserve_one` and `Displaced`, the reservation primitive the chunk and layout directories grow through without a borrow live across an allocator call. |
| `slot` | `SlotCell`, the free-list sentinel, the pool-size ceiling and the reference-count overflow guard. |
| `boxed`, `sync`, `rc`, `alloced` | The `Box`, `Arc`, `Rc` and `Alloc` handles. |
| `common` | Macros emitting the forwarding impls shared by all handles. |
| `coerce` | The `Coercion` token, `unsize()` and the `coerce!` macro. |
| `builder` | `PoolBuilder` and the typed pool's configuration validation. |
| `multi_builder` | `MultiPoolBuilder` and the multi pool's sizing and cap configuration. |
| `error` | `AllocError` and its private `ErrorKind`. |
| `pool_stats` | `PoolStats`, compiled only under the `stats` feature. |
| `atomic` | A re-export shim that swaps in `loom`'s atomics under `--cfg loom`. |

## Layering

Both pool forms are façades over one implementation. What separates them is a
single question — *is the slot geometry known at compile time?* — and that
question is answered by a type parameter rather than by duplicated code.

```text
   Pool<T, A>                              MultiPool<A>
   (public, typed)                         (public, any type)
        │                                       │
        │                                  layout directory
        │                                       │
        │                                  LayoutPool<A>
        │                                  (crate-private)
        ▼                                       ▼
   PoolInner<A, TypedGeometry<T>>          PoolInner<A, RuntimeGeometry>
        └──────────────────┬────────────────────┘
                           ▼
                       PoolCore
        (free-list head · pool refcount · teardown hook)
```

`PoolCore` has three fields and no generics, and is reached by pointer recovery
from any value pointer. Everything above it is generic over the geometry
provider, and the two providers differ only in whether their answers are
compile-time constants or loaded fields.

## The shape of the implementation

**Storage is chunked and immortal.** A pool owns a directory of chunks. A chunk
is one allocation holding a header followed by an array of slots, and a slot
holds one value, a `u32` reference count and a `u32` in-chunk index. Chunks
never move and are never freed individually; the pool releases them all at
teardown. That is what makes addresses stable and makes back-references from a
chunk to its pool safe to follow.

**Free slots are an intrusive, index-based list.** The head lives in `PoolCore`
as an atomic `u32` global slot index, and each free slot stores the next index
in the same word it uses as a reference count while occupied. Popping is
single-threaded, pushing is multi-producer, and both are lock-free.

**Reclamation is arithmetic, not a lookup.** A handle stores a pointer to the
value and nothing else. To free, it derives the slot geometry from the value's
own size and alignment, steps to the reference count and the index, steps back
to the chunk header, and reads the pool pointer stored there. No handle ever
needs to know which pool object produced it, which is what keeps handles one
pointer wide and what lets the multi pool put its router on the allocation path
alone.

**Two reference counts express two lifetimes.** For shared handles, the
per-slot count owns the value. The pool-level count in `PoolCore` owns the
memory: one unit for the pool object and one for each live detachable
allocation. When the pool object or an allocation's final detachable handle
releases the last unit, it runs the teardown hook stored in `PoolCore`, so
teardown works without ever naming the element type and without reaching the
pool object.

**One thread allocates at a time.** `Pool` is `Send` and not `Sync`, so a
shared reference to it cannot be shared across threads; the chunk directory and
the free-list pop are therefore single-threaded by construction, while frees
run concurrently from anywhere.

**Handles are thin and their shared surface is generated.** Four flavors cover
detached unique ownership, atomic and non-atomic shared ownership, and bound
unique ownership. Their common forwarding impls come from a small set of
macros in two variants, one for sized values and one that admits `?Sized`.

**Instrumentation is compiled out when unused.** Structural queries are always
available; the cumulative counters exist only under the `stats` feature, and
the `loom` configuration swaps the atomics for instrumented ones.
