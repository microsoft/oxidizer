// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/plurality/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/plurality/favicon.ico")]

//! A growable, fixed-slot object pool that hands out thin smart pointers.
//!
//! A [`Pool<T>`] allocates `T` values from reusable slots and returns
//! single-pointer-wide smart pointers that deref to `&T`. It grows on demand and
//! never moves a value once allocated, so the pointers stay valid until they are
//! dropped. There are four handle types, covering owned vs. shared and bound vs.
//! `'static`:
//!
//! - [`Box<T>`] — unique owner, `Send` when `T: Send` and `A: Send + Sync`, may
//!   outlive the pool.
//! - [`Alloc<'pool, T>`] — unique owner that borrows the pool; the cheapest
//!   handle, but `!Send` and cannot outlive the pool.
//! - [`Arc<T>`] — shared, atomically reference-counted, `Send + Sync` when
//!   `T: Send + Sync` and `A: Send + Sync`.
//! - [`Rc<T>`] — shared, non-atomically reference-counted, `!Send` (cheaper
//!   clone/drop than [`Arc`] for single-threaded sharing).
//!
//! All four deref to `&T`; [`Box`] and [`Alloc`] also give `&mut T`. Dropping a
//! handle runs `T`'s destructor and returns the slot to the pool.
//!
//! # Why a pool?
//!
//! Calling the global allocator for every short-lived object has real costs:
//! each `malloc`/`free` can take a lock, walk size-class free lists, and (under
//! churn) fragment the heap, while scattering objects across memory so traversals
//! miss cache. A pool front-loads one chunk allocation and then serves individual
//! objects from a free list — so the steady-state allocate/free path is a couple
//! of pointer ops with no global-allocator round trip (this crate measures about
//! 2.4 times faster than the system allocator on a graph-churn workload). Because
//! every value lives in a pre-allocated slot that never moves, related objects stay
//! close in memory and their addresses remain stable.
//!
//! Reach for a pool when:
//!
//! - **High-frequency allocate/free of one type.** Connection/buffer pools,
//!   packet or message buffers, work items, particles, audio voices — workloads
//!   that recycle many same-typed objects in a tight loop.
//! - **Stable addresses are required.** Graph nodes, intrusive lists, FFI
//!   handles, or self-referential structures that need a value's address to stay
//!   put. A `Vec<T>` reallocates and moves its elements on growth; a pool never
//!   does.
//! - **Predictable latency / bounded memory.** Capping growth (`max_chunks`)
//!   turns exhaustion into a graceful [`AllocError`] instead of an unbounded
//!   heap, and growth happens one chunk at a time with no `O(n)`
//!   reallocate-and-copy spike.
//! - **Shared ownership without per-object heap allocation.** [`Arc`]/[`Rc`]
//!   handles refcount within the pool's storage, so cloned references don't each
//!   carry a separate allocation.
//!
//! A pool is **not** the right tool for a few large, long-lived, differently-typed
//! allocations (just use the global allocator), or for objects that must all be
//! freed together with no individual reclamation (an arena like
//! [`multitude`][cr-multitude] is simpler). See the comparison table below for
//! how it relates to `slab`, `slotmap`, and other crates.
//!
//! [cr-multitude]: https://crates.io/crates/multitude
//!
//! # Concurrency model
//!
//! [`Pool<T>`] is `Send + !Sync`: allocating takes `&Pool`, so exactly one
//! thread allocates at a time (the whole pool can still be *moved* between
//! threads). The `Send` handles ([`Box`]/[`Arc`]) may be dropped from any thread;
//! the `!Send` handles ([`Alloc`]/[`Rc`]) stay on their thread.
//!
//! # Memory allocation
//!
//! As you allocate instances from a pool, the pool will allocate large chunks of memory from the
//! supplied allocator. The pool retains this memory until the overall pool is dropped.
//!
//! # Cargo features
//!
//! - **`std`** *(enabled by default)* — integrates with the standard library
//!   through [`allocator-api2`]'s `std` feature. The crate is otherwise
//!   `no_std` (it needs only [`alloc`]); disable default features to build for
//!   a `no_std` target.
//! - **`stats`** *(disabled by default)* — enables runtime allocation
//!   statistics: the `PoolStats` type and the `Pool::stats` method. The
//!   accounting counters are compiled in only when this feature is active, so
//!   leaving it off keeps the pool free of any tracking overhead.
//!
//! [`allocator-api2`]: https://crates.io/crates/allocator-api2
//!
//! # Type erasure
//!
//! [`Box<T>`], [`Arc<T>`], and [`Rc<T>`] are generic over `T: ?Sized`, so they can
//! hold an unsized value — a trait object or a slice — while the value stays in
//! its pool slot. A sized handle is converted with [`Box::unsize`] /
//! [`Arc::unsize`] / [`Rc::unsize`], which take a compiler-checked
//! [`Coercion`](struct@Coercion)
//! token:
//!
//! ```
//! use core::fmt::Debug;
//!
//! use plurality::{Box, Pool, coerce};
//!
//! let pool = Pool::<u32>::new();
//! let b = pool.alloc_box(7u32);
//! let erased = Box::unsize(b, coerce!(dyn Debug));
//! assert_eq!(format!("{erased:?}"), "7");
//! ```
//!
//! A sized handle stays exactly one pointer wide; the unsized forms carry the
//! extra pointer metadata (vtable or length) just like [`alloc::boxed::Box`], and
//! reclaim the slot from the value's runtime size and alignment on drop.
//!
//! # Comparison with other crates
//!
//! The closest crates in the ecosystem hand out *indices* or *keys* that only
//! deref while you hold the container, or recycle whole values behind a lock.
//! `plurality` instead returns thin smart pointers that deref (and, for
//! [`Arc`]/[`Rc`], share ownership) without the pool in hand.
//!
//! | Capability | [`plurality`][cr-plurality] | [`slab`][cr-slab] | [`sharded-slab`][cr-sharded-slab] | [`slotmap`][cr-slotmap] | [`object-pool`][cr-object-pool] | [`opool`][cr-opool] | [`deadpool`][cr-deadpool] | [`infinity-pool`][cr-infinity-pool] |
//! |---|---|---|---|---|---|---|---|---|
//! | Thin single-pointer handles (deref without the pool) | yes | no (index) | no (guard) | no (key) | no (guard) | no (guard) | no (guard) | yes |
//! | Individual free + slot reuse | yes | yes | yes | yes | yes | yes | yes | yes |
//! | Shared ownership ([`Arc`]/[`Rc`]) | yes | no | no | no | no | no | no | yes |
//! | Growable, chunked | yes | yes | yes | yes | yes | yes | yes | yes |
//! | Stable address (value never moves on grow) | yes | no | yes | no | no | no | no | yes |
//! | Thread safety | `Send + !Sync`, cross-thread frees | single-thread | `Send + Sync` | single-thread | `Send + Sync`, lock-based | `Send + Sync`, lock-free | `Send + Sync`, async | `Send + Sync`, or single-thread `Rc` |
//!
//! [cr-plurality]: https://crates.io/crates/plurality
//! [cr-slab]: https://crates.io/crates/slab
//! [cr-sharded-slab]: https://crates.io/crates/sharded-slab
//! [cr-slotmap]: https://crates.io/crates/slotmap
//! [cr-object-pool]: https://crates.io/crates/object-pool
//! [cr-opool]: https://crates.io/crates/opool
//! [cr-deadpool]: https://crates.io/crates/deadpool
//! [cr-infinity-pool]: https://crates.io/crates/infinity_pool
//!
//! # Examples
//!
//! ```
//! use plurality::Pool;
//!
//! let pool = Pool::<u32>::builder().chunk_size(64).build();
//!
//! // Unique, mutable ownership.
//! let mut a = pool.alloc_box(1);
//! *a += 10;
//! assert_eq!(*a, 11);
//!
//! // Shared ownership.
//! let b = pool.alloc_arc(2);
//! let b2 = b.clone();
//! assert_eq!(*b2, 2);
//! ```
//!
//! Bounding capacity and handling exhaustion without panicking:
//!
//! ```
//! use plurality::Pool;
//!
//! let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
//! let _held = pool.alloc_box(1);
//! // The single slot is taken, so this reports failure instead of panicking.
//! assert!(pool.try_alloc_box(2).is_err());
//! ```

extern crate alloc;

mod alloced;
mod atomic;
mod boxed;
mod builder;
mod chunk;
mod coerce;
mod common;
mod error;
mod pool;
#[cfg(feature = "stats")]
mod pool_stats;
mod rc;
mod slot;
mod sync;

pub use alloced::Alloc;
pub use boxed::Box;
pub use builder::PoolBuilder;
pub use coerce::Coercion;
pub use error::AllocError;
pub use pool::Pool;
#[cfg(feature = "stats")]
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
pub use pool_stats::PoolStats;
pub use rc::Rc;
pub use sync::Arc;
