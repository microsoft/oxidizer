<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Plurality Logo" width="96">

# Plurality

[![crate.io](https://img.shields.io/crates/v/plurality.svg)](https://crates.io/crates/plurality)
[![docs.rs](https://docs.rs/plurality/badge.svg)](https://docs.rs/plurality)
[![MSRV](https://img.shields.io/crates/msrv/plurality)](https://crates.io/crates/plurality)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A growable, fixed-slot object pool that hands out thin smart pointers.

A [`Pool<T>`][__link0] allocates `T` values from reusable slots and returns
single-pointer-wide smart pointers that deref to `&T`. It grows on demand and
never moves a value once allocated, so the pointers stay valid until they are
dropped. The handle types cover owned vs. shared and bound vs. `'static`:

* [`Box<T>`][__link1] — unique owner, `Send` when `T: Send` and `A: Send + Sync`, may
  outlive the pool.
* [`Alloc<'pool, T>`][__link2] — unique owner that borrows the pool; the cheapest
  handle, but `!Send` and cannot outlive the pool.
* [`Arc<T>`][__link3] — shared, atomically reference-counted, `Send + Sync` when
  `T: Send + Sync` and `A: Send + Sync`.
* [`Rc<T>`][__link4] — shared, non-atomically reference-counted, `!Send` (cheaper
  clone/drop than [`Arc`][__link5] for single-threaded sharing).

Every handle derefs to `&T`; [`Box`][__link6] and [`Alloc`][__link7] also give `&mut T`. Dropping a
handle runs `T`’s destructor and returns the slot to the pool.

A [`MultiPool`][__link8] moves the element type from the pool to each allocation, so
one pool object backs values of many types. It creates an internal layout
pool for each distinct slot shape it sees and routes each value to the pool
serving that shape, so a value occupies exactly the space a pool dedicated
to its type would give it, while handing out the same handles with the same
guarantees.

Use [`Pool<T>`][__link9] for a working set that repeatedly allocates one value type,
and [`MultiPool`][__link10] for heterogeneous recycled values. Both flavors suit
stable-address data structures and workloads that need a capacity limit.
Slots are reused without a chunk-allocation call. `max_chunks` bounds a
[`Pool<T>`][__link11] or one [`MultiPool`][__link12] layout pool; bounding aggregate
[`MultiPool`][__link13] growth also requires `max_layouts`, and total memory depends
on the layouts and effective chunk sizes. Prefer a general allocator for
long-lived values or an arena when values can all be reclaimed together.

## Performance

See [`PERF.md`][__link14]
for measured wall-clock numbers: the cost of each handle type, the cost of
serving many types from one pool, a churn workload against the system
allocator, and head-to-head comparisons with the other Rust pooling crates.

## Concurrency model

[`Pool<T>`][__link15] is `Send + !Sync`: allocating takes `&Pool`, so exactly one
thread allocates at a time (the whole pool can still be *moved* between
threads). The `Send` handles ([`Box`][__link16]/[`Arc`][__link17]) may be dropped from any thread;
the `!Send` handles ([`Alloc`][__link18]/[`Rc`][__link19]) stay on their thread.

Moving a pool is independent of moving its values: [`Pool<T>`][__link20] is `Send`
when the allocator is, whatever `T` is, because a pool owns no values and
offers no way to reach one. Thread mobility for values is carried entirely
by the handles. [`MultiPool`][__link21] follows the same rule, which is what lets one
pool hold values of types with different thread affinities at once.

Serving several threads from one pool is ordinary: wrap it in a `Mutex` and
keep only allocation inside the critical section. Dropping a detachable
handle ([`Box`][__link22]/[`Arc`][__link23]/[`Rc`][__link24]) needs no lock, so reclamation runs unlocked
and in parallel. [`Alloc`][__link25] is the exception: it borrows the pool, so the
guard must outlive it.

## Memory allocation

The pool allocates chunks from the supplied allocator and retains them until
teardown.

## Cargo features

* **`std`** *(enabled by default)* — integrates with the standard library
  through [`allocator-api2`][__link26]’s `std` feature. The crate is otherwise
  `no_std` (it needs only [`alloc`][__link27]); disable default features to build for
  a `no_std` target.
* **`stats`** *(disabled by default)* — enables runtime allocation
  statistics: the `PoolStats` type and the `Pool::stats` and
  `MultiPool::stats` methods. The accounting counters are compiled in only
  when this feature is active, so leaving it off keeps the pool free of any
  tracking overhead.

## Type erasure

[`Box<T>`][__link28], [`Arc<T>`][__link29], and [`Rc<T>`][__link30] are generic over `T: ?Sized`, so they can
hold an unsized value — a trait object or a slice — while the value stays in
its pool slot. A sized handle is converted with [`Box::unsize`][__link31] /
[`Arc::unsize`][__link32] / [`Rc::unsize`][__link33], which take a compiler-checked
[`Coercion`][__link34]
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
extra pointer metadata (vtable or length) just like [`alloc::boxed::Box`][__link35], and
reclaim the slot from the value’s runtime size and alignment on drop.

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

Runnable programs covering larger scenarios:

* [`pool_basic`][__link36]: The handle flavors, address stability, and slot reuse.
* [`pool_across_threads`][__link37]: Sharing a pool through a `Mutex` and reclaiming slots from worker threads.
* [`multi_pool_basic`][__link38]: Values of unrelated types in one pool, and per-layout capacity.
* [`multi_pool_dyn_dispatch`][__link39]: A pipeline of differently sized trait objects backed by one pool.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/plurality">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbbdeMU7IMMGYbOQEy8TTZ9ZgbRc-RXf2PBOgbI0s5F2a98nFhZIGCaXBsdXJhbGl0eWUwLjIuMg
 [__link0]: https://docs.rs/plurality/0.2.2/plurality/?search=Pool
 [__link1]: https://docs.rs/plurality/0.2.2/plurality/?search=Box
 [__link10]: https://docs.rs/plurality/0.2.2/plurality/?search=MultiPool
 [__link11]: https://docs.rs/plurality/0.2.2/plurality/?search=Pool
 [__link12]: https://docs.rs/plurality/0.2.2/plurality/?search=MultiPool
 [__link13]: https://docs.rs/plurality/0.2.2/plurality/?search=MultiPool
 [__link14]: https://github.com/microsoft/oxidizer/blob/main/crates/plurality/docs/PERF.md
 [__link15]: https://docs.rs/plurality/0.2.2/plurality/?search=Pool
 [__link16]: https://docs.rs/plurality/0.2.2/plurality/?search=Box
 [__link17]: https://docs.rs/plurality/0.2.2/plurality/?search=Arc
 [__link18]: https://docs.rs/plurality/0.2.2/plurality/?search=Alloc
 [__link19]: https://docs.rs/plurality/0.2.2/plurality/?search=Rc
 [__link2]: https://docs.rs/plurality/0.2.2/plurality/?search=Alloc
 [__link20]: https://docs.rs/plurality/0.2.2/plurality/?search=Pool
 [__link21]: https://docs.rs/plurality/0.2.2/plurality/?search=MultiPool
 [__link22]: https://docs.rs/plurality/0.2.2/plurality/?search=Box
 [__link23]: https://docs.rs/plurality/0.2.2/plurality/?search=Arc
 [__link24]: https://docs.rs/plurality/0.2.2/plurality/?search=Rc
 [__link25]: https://docs.rs/plurality/0.2.2/plurality/?search=Alloc
 [__link26]: https://crates.io/crates/allocator-api2
 [__link27]: https://doc.rust-lang.org/stable/alloc
 [__link28]: https://docs.rs/plurality/0.2.2/plurality/?search=Box
 [__link29]: https://docs.rs/plurality/0.2.2/plurality/?search=Arc
 [__link3]: https://docs.rs/plurality/0.2.2/plurality/?search=Arc
 [__link30]: https://docs.rs/plurality/0.2.2/plurality/?search=Rc
 [__link31]: https://docs.rs/plurality/0.2.2/plurality/?search=Box::unsize
 [__link32]: https://docs.rs/plurality/0.2.2/plurality/?search=Arc::unsize
 [__link33]: https://docs.rs/plurality/0.2.2/plurality/?search=Rc::unsize
 [__link34]: https://docs.rs/plurality/latest/plurality/struct.Coercion.html
 [__link35]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link36]: https://github.com/microsoft/oxidizer/blob/main/crates/plurality/examples/pool_basic.rs
 [__link37]: https://github.com/microsoft/oxidizer/blob/main/crates/plurality/examples/pool_across_threads.rs
 [__link38]: https://github.com/microsoft/oxidizer/blob/main/crates/plurality/examples/multi_pool_basic.rs
 [__link39]: https://github.com/microsoft/oxidizer/blob/main/crates/plurality/examples/multi_pool_dyn_dispatch.rs
 [__link4]: https://docs.rs/plurality/0.2.2/plurality/?search=Rc
 [__link5]: https://docs.rs/plurality/0.2.2/plurality/?search=Arc
 [__link6]: https://docs.rs/plurality/0.2.2/plurality/?search=Box
 [__link7]: https://docs.rs/plurality/0.2.2/plurality/?search=Alloc
 [__link8]: https://docs.rs/plurality/0.2.2/plurality/?search=MultiPool
 [__link9]: https://docs.rs/plurality/0.2.2/plurality/?search=Pool
