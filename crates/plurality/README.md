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

## Memory allocation

As you allocate instances from a pool, the pool will allocate large chunks of memory from the
supplied allocator. The pool retains this memory until the overall pool is dropped.

## Cargo features

* **`std`** *(enabled by default)* — integrates with the standard library
  through [`allocator-api2`][__link17]’s `std` feature. The crate is otherwise
  `no_std` (it needs only [`alloc`][__link18]); disable default features to build for
  a `no_std` target.
* **`stats`** *(disabled by default)* — enables runtime allocation
  statistics: the `PoolStats` type and the `Pool::stats` method. The
  accounting counters are compiled in only when this feature is active, so
  leaving it off keeps the pool free of any tracking overhead.

## Type erasure

[`Box<T>`][__link19], [`Arc<T>`][__link20], and [`Rc<T>`][__link21] are generic over `T: ?Sized`, so they can
hold an unsized value — a trait object or a slice — while the value stays in
its pool slot. A sized handle is converted with [`Box::unsize`][__link22] /
[`Arc::unsize`][__link23] / [`Rc::unsize`][__link24], which take a compiler-checked
[`Coercion`][__link25]
token:

```rust
use core::fmt::Debug;

use plurality::{Box, Pool, coerce};

let pool = Pool::<u32>::new();
let b = pool.alloc_box(7u32);
let erased = Box::unsize(b, coerce!(dyn Debug));
assert_eq!(format!("{erased:?}"), "7");
```

A sized handle stays exactly one pointer wide; the unsized forms carry the
extra pointer metadata (vtable or length) just like [`alloc::boxed::Box`][__link26], and
reclaim the slot from the value’s runtime size and alignment on drop.

## Comparison with other crates

The closest crates in the ecosystem hand out *indices* or *keys* that only
deref while you hold the container, or recycle whole values behind a lock.
`plurality` instead returns thin smart pointers that deref (and, for
[`Arc`][__link27]/[`Rc`][__link28], share ownership) without the pool in hand.

|Capability|[`plurality`][__link29]|[`slab`][__link30]|[`sharded-slab`][__link31]|[`slotmap`][__link32]|[`object-pool`][__link33]|[`opool`][__link34]|[`deadpool`][__link35]|[`infinity-pool`][__link36]|
|----------|-----------|------|--------------|---------|-------------|-------|----------|---------------|
|Thin single-pointer handles (deref without the pool)|yes|no (index)|no (guard)|no (key)|no (guard)|no (guard)|no (guard)|yes|
|Individual free + slot reuse|yes|yes|yes|yes|yes|yes|yes|yes|
|Shared ownership ([`Arc`][__link37]/[`Rc`][__link38])|yes|no|no|no|no|no|no|yes|
|Growable, chunked|yes|yes|yes|yes|yes|yes|yes|yes|
|Stable address (value never moves on grow)|yes|no|yes|no|no|no|no|yes|
|Thread safety|`Send + !Sync`, cross-thread frees|single-thread|`Send + Sync`|single-thread|`Send + Sync`, lock-based|`Send + Sync`, lock-free|`Send + Sync`, async|`Send + Sync`, or single-thread `Rc`|

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

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG5fMTMzy4UGnG1kTnRsEXYRXG8-JQrhebgRtG5v01AZ0Vm6uYWSBgmlwbHVyYWxpdHllMC4xLjE
 [__link0]: https://docs.rs/plurality/0.1.1/plurality/?search=Pool
 [__link1]: https://docs.rs/plurality/0.1.1/plurality/?search=Box
 [__link10]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc
 [__link11]: https://crates.io/crates/multitude
 [__link12]: https://docs.rs/plurality/0.1.1/plurality/?search=Pool
 [__link13]: https://docs.rs/plurality/0.1.1/plurality/?search=Box
 [__link14]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
 [__link15]: https://docs.rs/plurality/0.1.1/plurality/?search=Alloc
 [__link16]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc
 [__link17]: https://crates.io/crates/allocator-api2
 [__link18]: https://doc.rust-lang.org/stable/alloc
 [__link19]: https://docs.rs/plurality/0.1.1/plurality/?search=Box
 [__link2]: https://docs.rs/plurality/0.1.1/plurality/?search=Alloc
 [__link20]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
 [__link21]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc
 [__link22]: https://docs.rs/plurality/0.1.1/plurality/?search=Box::unsize
 [__link23]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc::unsize
 [__link24]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc::unsize
 [__link25]: https://docs.rs/plurality/latest/plurality/struct.Coercion.html
 [__link26]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link27]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
 [__link28]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc
 [__link29]: https://crates.io/crates/plurality
 [__link3]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
 [__link30]: https://crates.io/crates/slab
 [__link31]: https://crates.io/crates/sharded-slab
 [__link32]: https://crates.io/crates/slotmap
 [__link33]: https://crates.io/crates/object-pool
 [__link34]: https://crates.io/crates/opool
 [__link35]: https://crates.io/crates/deadpool
 [__link36]: https://crates.io/crates/infinity_pool
 [__link37]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
 [__link38]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc
 [__link4]: https://docs.rs/plurality/0.1.1/plurality/?search=Rc
 [__link5]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
 [__link6]: https://docs.rs/plurality/0.1.1/plurality/?search=Box
 [__link7]: https://docs.rs/plurality/0.1.1/plurality/?search=Alloc
 [__link8]: https://docs.rs/plurality/0.1.1/plurality/?search=AllocError
 [__link9]: https://docs.rs/plurality/0.1.1/plurality/?search=Arc
