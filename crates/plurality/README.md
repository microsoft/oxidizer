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

A [`BlindPool`][__link8] moves the element type from the pool to the allocation, so
one pool object backs values of any type. It hands out the same four
handles with the same guarantees, routing each value to the internal pool
serving its layout.

Pools suit frequently recycled values of one type, stable-address data
structures, and workloads that need a capacity limit. Slots are reused
without a backing-allocator call, and `max_chunks` bounds growth. Prefer a
general allocator for long-lived values or an arena when values can all be
reclaimed together.

## Concurrency model

[`Pool<T>`][__link9] is `Send + !Sync`: allocating takes `&Pool`, so exactly one
thread allocates at a time (the whole pool can still be *moved* between
threads). The `Send` handles ([`Box`][__link10]/[`Arc`][__link11]) may be dropped from any thread;
the `!Send` handles ([`Alloc`][__link12]/[`Rc`][__link13]) stay on their thread.

Moving a pool is independent of moving its values: [`Pool<T>`][__link14] is `Send`
when the allocator is, whatever `T` is, because a pool owns no values and
offers no way to reach one. Thread mobility for values is carried entirely
by the handles. [`BlindPool`][__link15] follows the same rule, which is what lets one
pool hold values of types with different thread affinities at once.

Serving several threads from one pool is ordinary: wrap it in a `Mutex` and
keep only allocation inside the critical section. Dropping a detachable
handle ([`Box`][__link16]/[`Arc`][__link17]/[`Rc`][__link18]) needs no lock, so reclamation runs unlocked
and in parallel. [`Alloc`][__link19] is the exception: it borrows the pool, so the
guard must outlive it.

## Memory allocation

The pool allocates chunks from the supplied allocator and retains them until
teardown.

## Cargo features

* **`std`** *(enabled by default)* — integrates with the standard library
  through [`allocator-api2`][__link20]’s `std` feature. The crate is otherwise
  `no_std` (it needs only [`alloc`][__link21]); disable default features to build for
  a `no_std` target.
* **`stats`** *(disabled by default)* — enables runtime allocation
  statistics: the `PoolStats` type and the `Pool::stats` and
  `BlindPool::stats` methods. The accounting counters are compiled in only
  when this feature is active, so leaving it off keeps the pool free of any
  tracking overhead.

## Type erasure

[`Box<T>`][__link22], [`Arc<T>`][__link23], and [`Rc<T>`][__link24] are generic over `T: ?Sized`, so they can
hold an unsized value — a trait object or a slice — while the value stays in
its pool slot. A sized handle is converted with [`Box::unsize`][__link25] /
[`Arc::unsize`][__link26] / [`Rc::unsize`][__link27], which take a compiler-checked
[`Coercion`][__link28]
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
extra pointer metadata (vtable or length) just like [`alloc::boxed::Box`][__link29], and
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


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/plurality">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbtlcZo7-lZvobWmhHW7M5AVgbyk5BRmUDLiYb-r0Q02M8_kdhZIGCaXBsdXJhbGl0eWUwLjIuMQ
 [__link0]: https://docs.rs/plurality/0.2.1/plurality/?search=Pool
 [__link1]: https://docs.rs/plurality/0.2.1/plurality/?search=Box
 [__link10]: https://docs.rs/plurality/0.2.1/plurality/?search=Box
 [__link11]: https://docs.rs/plurality/0.2.1/plurality/?search=Arc
 [__link12]: https://docs.rs/plurality/0.2.1/plurality/?search=Alloc
 [__link13]: https://docs.rs/plurality/0.2.1/plurality/?search=Rc
 [__link14]: https://docs.rs/plurality/0.2.1/plurality/?search=Pool
 [__link15]: https://docs.rs/plurality/0.2.1/plurality/?search=BlindPool
 [__link16]: https://docs.rs/plurality/0.2.1/plurality/?search=Box
 [__link17]: https://docs.rs/plurality/0.2.1/plurality/?search=Arc
 [__link18]: https://docs.rs/plurality/0.2.1/plurality/?search=Rc
 [__link19]: https://docs.rs/plurality/0.2.1/plurality/?search=Alloc
 [__link2]: https://docs.rs/plurality/0.2.1/plurality/?search=Alloc
 [__link20]: https://crates.io/crates/allocator-api2
 [__link21]: https://doc.rust-lang.org/stable/alloc
 [__link22]: https://docs.rs/plurality/0.2.1/plurality/?search=Box
 [__link23]: https://docs.rs/plurality/0.2.1/plurality/?search=Arc
 [__link24]: https://docs.rs/plurality/0.2.1/plurality/?search=Rc
 [__link25]: https://docs.rs/plurality/0.2.1/plurality/?search=Box::unsize
 [__link26]: https://docs.rs/plurality/0.2.1/plurality/?search=Arc::unsize
 [__link27]: https://docs.rs/plurality/0.2.1/plurality/?search=Rc::unsize
 [__link28]: https://docs.rs/plurality/latest/plurality/struct.Coercion.html
 [__link29]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link3]: https://docs.rs/plurality/0.2.1/plurality/?search=Arc
 [__link4]: https://docs.rs/plurality/0.2.1/plurality/?search=Rc
 [__link5]: https://docs.rs/plurality/0.2.1/plurality/?search=Arc
 [__link6]: https://docs.rs/plurality/0.2.1/plurality/?search=Box
 [__link7]: https://docs.rs/plurality/0.2.1/plurality/?search=Alloc
 [__link8]: https://docs.rs/plurality/0.2.1/plurality/?search=BlindPool
 [__link9]: https://docs.rs/plurality/0.2.1/plurality/?search=Pool
