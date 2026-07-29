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
//! Pools suit frequently recycled values of one type, stable-address data
//! structures, and workloads that need a capacity limit. Slots are reused
//! without a backing-allocator call, and `max_chunks` bounds growth. Prefer a
//! general allocator for heterogeneous, long-lived values or an arena when
//! values can all be reclaimed together.
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
//! The pool allocates chunks from the supplied allocator and retains them until
//! teardown.
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
//! [`Coercion`](https://docs.rs/plurality/latest/plurality/struct.Coercion.html)
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
