<div align="center">
 <img src="./logo.png" alt="Plurality Logo" width="96">

# Plurality

[![crate.io](https://img.shields.io/crates/v/plurality.svg)](https://crates.io/crates/plurality)
[![docs.rs](https://docs.rs/plurality/badge.svg)](https://docs.rs/plurality)
[![MSRV](https://img.shields.io/crates/msrv/plurality)](https://crates.io/crates/plurality)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A growable, fixed-slot object pool that hands out thin smart pointers.

A [`Pool<T>`][__link0] allocates `T` values from reusable slots and returns
single-pointer-wide smart pointers that deref to `&T`. It grows on demand and
never moves a value once allocated, so the pointers stay valid until they are
dropped. There are four handle types, covering owned vs. shared and bound vs.
`'static`:

* [`Box<T>`][__link1] — unique owner, `Send` when `T: Send` and `A: Send + Sync`, may
  outlive the pool.
* [`Alloc<'pool, T>`][__link2] — unique owner that borrows the pool; the cheapest
  handle, but `!Send` and cannot outlive the pool.
* [`Arc<T>`][__link3] — shared, atomically reference-counted, `Send + Sync` when
  `T: Send + Sync` and `A: Send + Sync`.
* [`Rc<T>`][__link4] — shared, non-atomically reference-counted, `!Send` (cheaper
  clone/drop than [`Arc`][__link5] for single-threaded sharing).

All four deref to `&T`; [`Box`][__link6] and [`Alloc`][__link7] also give `&mut T`. Dropping a
handle runs `T`’s destructor and returns the slot to the pool.

## Why a pool?

Calling the global allocator for every short-lived object has real costs:
each `malloc`/`free` can take a lock, walk size-class free lists, and (under
churn) fragment the heap, while scattering objects across memory so traversals
miss cache. A pool front-loads one chunk allocation and then serves individual
objects from a free list — so the steady-state allocate/free path is a couple
of pointer ops with no global-allocator round trip (this crate measures about
2.4 times faster than the system allocator on a graph-churn workload). Because
every value lives in a pre-allocated slot that never moves, related objects stay
close in memory and their addresses remain stable.

Reach for a pool when:

* **High-frequency allocate/free of one type.** Connection/buffer pools,
  packet or message buffers, work items, particles, audio voices — workloads
  that recycle many same-typed objects in a tight loop.
* **Stable addresses are required.** Graph nodes, intrusive lists, FFI
  handles, or self-referential structures that need a value’s address to stay
  put. A `Vec<T>` reallocates and moves its elements on growth; a pool never
  does.
* **Predictable latency / bounded memory.** Capping growth (`max_chunks`)
  turns exhaustion into a graceful [`AllocError`][__link8] instead of an unbounded
  heap, and growth happens one chunk at a time with no `O(n)`
  reallocate-and-copy spike.
* **Shared ownership without per-object heap allocation.** [`Arc`][__link9]/[`Rc`][__link10]
  handles refcount within the pool’s storage, so cloned references don’t each
  carry a separate allocation.

A pool is **not** the right tool for a few large, long-lived, differently-typed
allocations (just use the global allocator), or for objects that must all be
freed together with no individual reclamation (an arena like
[`multitude`][__link11] is simpler). See the comparison table below for
how it relates to `slab`, `slotmap`, and other crates.

## Concurrency model

[`Pool<T>`][__link12] is `Send + !Sync`: allocating takes `&Pool`, so exactly one
thread allocates at a time (the whole pool can still be *moved* between
threads). The `Send` handles ([`Box`][__link13]/[`Arc`][__link14]) may be dropped from any thread;
the `!Send` handles ([`Alloc`][__link15]/[`Rc`][__link16]) stay on their thread.

## Comparison with other crates

The closest crates in the ecosystem hand out *indices* or *keys* that only
deref while you hold the container, or recycle whole values behind a lock.
`plurality` instead returns thin smart pointers that deref (and, for
[`Arc`][__link17]/[`Rc`][__link18], share ownership) without the pool in hand.

|Capability|[`plurality`][__link19]|[`slab`][__link20]|[`sharded-slab`][__link21]|[`slotmap`][__link22]|[`object-pool`][__link23]|[`opool`][__link24]|[`deadpool`][__link25]|
|----------|-----------|------|--------------|---------|-------------|-------|----------|
|Thin single-pointer handles (deref without the pool)|yes|no (index)|no (guard)|no (key)|no (guard)|no (guard)|no (guard)|
|Individual free + slot reuse|yes|yes|yes|yes|yes|yes|yes|
|Shared ownership ([`Arc`][__link26]/[`Rc`][__link27])|yes|no|no|no|no|no|no|
|Growable, chunked|yes|yes|yes|yes|yes|yes|yes|
|Stable address (value never moves on grow)|yes|no|yes|no|no|no|no|
|Thread safety|`Send + !Sync`, cross-thread frees|single-thread|`Send + Sync`|single-thread|`Send + Sync`, lock-based|`Send + Sync`, lock-free|`Send + Sync`, async|

## Examples

```rust
use plurality::Pool;

let pool = Pool::<u32>::builder().chunk_size(64).build();

// Unique, mutable ownership.
let mut a = pool.alloc_box(1);
*a += 10;
assert_eq!(*a, 11);

// Shared ownership.
let b = pool.alloc_arc(2);
let b2 = b.clone();
assert_eq!(*b2, 2);
```

Bounding capacity and handling exhaustion without panicking:

```rust
use plurality::Pool;

let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
let _held = pool.alloc_box(1);
// The single slot is taken, so this reports failure instead of panicking.
assert!(pool.try_alloc_box(2).is_err());
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/plurality">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbXVSowJfMZPUbgvaJBRDISOEbYNUm3j2gevYbk5D3LFHDmQVhZIGCaXBsdXJhbGl0eWUwLjEuMA
 [__link0]: https://docs.rs/plurality/0.1.0/plurality/?search=Pool
 [__link1]: https://docs.rs/plurality/0.1.0/plurality/?search=Box
 [__link10]: https://docs.rs/plurality/0.1.0/plurality/?search=Rc
 [__link11]: https://crates.io/crates/multitude
 [__link12]: https://docs.rs/plurality/0.1.0/plurality/?search=Pool
 [__link13]: https://docs.rs/plurality/0.1.0/plurality/?search=Box
 [__link14]: https://docs.rs/plurality/0.1.0/plurality/?search=Arc
 [__link15]: https://docs.rs/plurality/0.1.0/plurality/?search=Alloc
 [__link16]: https://docs.rs/plurality/0.1.0/plurality/?search=Rc
 [__link17]: https://docs.rs/plurality/0.1.0/plurality/?search=Arc
 [__link18]: https://docs.rs/plurality/0.1.0/plurality/?search=Rc
 [__link19]: https://crates.io/crates/plurality
 [__link2]: https://docs.rs/plurality/0.1.0/plurality/?search=Alloc
 [__link20]: https://crates.io/crates/slab
 [__link21]: https://crates.io/crates/sharded-slab
 [__link22]: https://crates.io/crates/slotmap
 [__link23]: https://crates.io/crates/object-pool
 [__link24]: https://crates.io/crates/opool
 [__link25]: https://crates.io/crates/deadpool
 [__link26]: https://docs.rs/plurality/0.1.0/plurality/?search=Arc
 [__link27]: https://docs.rs/plurality/0.1.0/plurality/?search=Rc
 [__link3]: https://docs.rs/plurality/0.1.0/plurality/?search=Arc
 [__link4]: https://docs.rs/plurality/0.1.0/plurality/?search=Rc
 [__link5]: https://docs.rs/plurality/0.1.0/plurality/?search=Arc
 [__link6]: https://docs.rs/plurality/0.1.0/plurality/?search=Box
 [__link7]: https://docs.rs/plurality/0.1.0/plurality/?search=Alloc
 [__link8]: https://docs.rs/plurality/0.1.0/plurality/?search=AllocError
 [__link9]: https://docs.rs/plurality/0.1.0/plurality/?search=Arc
