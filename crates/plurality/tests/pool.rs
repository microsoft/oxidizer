// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::clone_on_ref_ptr,
    clippy::unwrap_used,
    clippy::assertions_on_result_states,
    clippy::cast_possible_truncation,
    clippy::collection_is_never_read,
    clippy::items_after_statements,
    clippy::many_single_char_names,
    clippy::borrow_as_ptr,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    reason = "test code"
)]

//! Tests for the `Pool` and `PoolBuilder`: construction, introspection, chunk
//! growth, slot reuse, bounded-pool exhaustion, allocator failure, teardown,
//! and concurrent frees.

mod common;

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use allocator_api2::alloc::Global;
use common::DropCounter;
use plurality::{AllocError, Pool};

/// A full, single-slot bounded pool plus the handle keeping it full.
fn full_pool() -> (Pool<u32>, plurality::Box<u32>) {
    let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    let held = pool.alloc_box(0);
    (pool, held)
}

// ── construction & introspection ─────────────────────────────────────────

#[test]
fn constructors_and_builder() {
    let _ = Pool::<u32>::new();
    let _ = Pool::<u32>::default();
    // The builder is obtained from its target type, not constructed directly.
    let _ = Pool::<u32>::builder();
    // allocator() swaps the allocator type; build with the global one.
    let pool = Pool::<u32>::builder().chunk_size(8).allocator(Global).build();
    let b = pool.alloc_box(1);
    assert_eq!(*b, 1);
}

#[test]
fn introspection() {
    let bounded = Pool::<u32>::builder().chunk_size(4).max_chunks(3).build();
    assert_eq!(bounded.chunk_size(), 4);
    assert_eq!(bounded.max_chunks(), Some(3));
    assert_eq!(bounded.max_capacity(), Some(12));
    assert_eq!(bounded.chunks_allocated(), 0);
    assert_eq!(bounded.capacity(), 0);
    assert!(bounded.is_empty());
    assert_eq!(bounded.available(), 0);

    let a = bounded.alloc_box(1);
    let b = bounded.alloc_arc(2);
    assert_eq!(bounded.len(), 2);
    assert!(!bounded.is_empty());
    assert_eq!(bounded.chunks_allocated(), 1);
    assert_eq!(bounded.capacity(), 4);
    assert_eq!(bounded.available(), 2);
    drop((a, b));

    let unbounded = Pool::<u32>::new();
    assert_eq!(unbounded.max_chunks(), None);
    assert_eq!(unbounded.max_capacity(), None);
}

#[test]
fn closure_and_uninit_constructors() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    assert_eq!(*pool.alloc_box_with(|| 1), 1);
    assert_eq!(*pool.alloc_arc_with(|| 2), 2);
    assert_eq!(*pool.alloc_with(|| 3), 3);
    assert_eq!(*pool.alloc_rc_with(|| 4), 4);

    let mut ub = pool.alloc_uninit_box();
    ub.write(5);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ub.assume_init() }, 5);

    let mut ua = pool.alloc_uninit_arc();
    plurality::Arc::get_mut(&mut ua).unwrap().write(6);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ua.assume_init() }, 6);

    let mut ul = pool.alloc_uninit();
    ul.write(7);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ul.assume_init() }, 7);

    let mut ur = pool.alloc_uninit_rc();
    plurality::Rc::get_mut(&mut ur).unwrap().write(8);
    // SAFETY: written just above.
    assert_eq!(*unsafe { ur.assume_init() }, 8);
}

// ── growth & slot reuse ──────────────────────────────────────────────────

#[test]
fn growth_across_chunks() {
    let pool = Pool::<usize>::builder().chunk_size(4).build();
    assert_eq!(pool.chunk_size(), 4);
    let mut held = Vec::new();
    for i in 0..10 {
        held.push(pool.alloc_box(i));
    }
    assert_eq!(pool.chunks_allocated(), 3); // ceil(10/4)
    assert_eq!(pool.capacity(), 12);
    assert_eq!(pool.len(), 10);
    for (i, b) in held.iter().enumerate() {
        assert_eq!(**b, i);
    }
}

#[test]
fn chunk_size_rounds_to_power_of_two() {
    let pool = Pool::<u8>::builder().chunk_size(5).build();
    assert_eq!(pool.chunk_size(), 8);
}

#[test]
fn slot_reuse_keeps_capacity() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();
    let b = pool.alloc_box(1);
    drop(b);
    let b = pool.alloc_box(2);
    let _c = pool.alloc_box(3);
    assert_eq!(*b, 2);
    // First slot was reused; we never needed more than one chunk.
    assert_eq!(pool.chunks_allocated(), 1);
}

#[test]
fn single_slot_chunks_grow_one_chunk_per_allocation() {
    // chunk_size == 1 exercises the `grow` path that reserves the only slot and
    // splices nothing.
    let pool = Pool::<u32>::builder().chunk_size(1).build();
    let a = pool.alloc_box(10);
    let b = pool.alloc_box(20);
    let c = pool.alloc_box(30);
    assert_eq!((*a, *b, *c), (10, 20, 30));
    assert_eq!(pool.chunks_allocated(), 3);
}

// ── exhaustion ───────────────────────────────────────────────────────────

#[test]
fn bounded_pool_reports_full_and_recovers() {
    let pool = Pool::<u32>::builder().chunk_size(2).max_chunks(1).build();
    assert_eq!(pool.max_capacity(), Some(2));
    let a = pool.alloc_box(1);
    let b = pool.alloc_box(2);
    // Pool is full now.
    assert!(pool.try_alloc_box(3).is_err());
    drop(a);
    // A slot freed; allocation should succeed again.
    let c = pool.alloc_box(4);
    assert_eq!(*c, 4);
    drop((b, c));
}

#[test]
fn rejected_value_is_dropped_when_full() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(1).max_chunks(1).build();
    let _held = pool.alloc_box(DropCounter(counter.clone()));
    // The pool is full: the rejected value's destructor must run (it is not
    // handed back to the caller).
    assert!(pool.try_alloc_box(DropCounter(counter.clone())).is_err());
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    // A `_with` closure must not run at all when the pool is full.
    let mut called = false;
    assert!(
        pool.try_alloc_box_with(|| {
            called = true;
            DropCounter(counter.clone())
        })
        .is_err()
    );
    assert!(!called);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn try_alloc_with_does_not_call_closure_when_full() {
    let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    let _a = pool.alloc_box(1);
    let called = std::cell::Cell::new(false);
    let res = pool.try_alloc_box_with(|| {
        called.set(true);
        99
    });
    assert!(res.is_err());
    assert!(!called.get(), "closure must not run when the pool is full");
}

#[test]
fn closure_panic_returns_slot_to_pool() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    // A panicking `_with` closure must not leak the reserved slot: on a
    // capacity-1 bounded pool, the slot has to return to the free list so a
    // subsequent allocation still succeeds (a leak would exhaust the pool).
    fn check(alloc_panics: impl Fn(&Pool<u32>)) {
        let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();

        let panicked = catch_unwind(AssertUnwindSafe(|| alloc_panics(&pool)));
        assert!(panicked.is_err(), "the closure was expected to panic");
        assert_eq!(pool.len(), 0, "the panicked allocation must not stay live");

        let recovered = pool.try_alloc_box(7);
        assert!(recovered.is_ok(), "the slot was not returned to the pool after the panic");
        assert_eq!(*recovered.unwrap(), 7);
    }

    check(|p| {
        let _ = p.try_alloc_box_with(|| panic!("boom"));
    });
    check(|p| {
        let _ = p.try_alloc_arc_with(|| panic!("boom"));
    });
    check(|p| {
        let _ = p.try_alloc_with(|| panic!("boom"));
    });
    check(|p| {
        let _ = p.try_alloc_rc_with(|| panic!("boom"));
    });
}

#[test]
fn try_alloc_uninit_reports_full() {
    let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    let _a = pool.alloc_uninit_box();
    assert!(pool.try_alloc_uninit_box().is_err());
}

#[test]
fn try_alloc_reports_full() {
    let (pool, _held) = full_pool();

    assert!(pool.try_alloc_box(11).is_err());
    assert!(pool.try_alloc_arc(12).is_err());
    assert!(pool.try_alloc(13).is_err());
    assert!(pool.try_alloc_rc(14).is_err());

    assert!(pool.try_alloc_box_with(|| 21).is_err());
    assert!(pool.try_alloc_arc_with(|| 22).is_err());
    assert!(pool.try_alloc_with(|| 23).is_err());
    assert!(pool.try_alloc_rc_with(|| 24).is_err());

    assert!(pool.try_alloc_uninit_box().is_err());
    assert!(pool.try_alloc_uninit_arc().is_err());
    assert!(pool.try_alloc_uninit().is_err());
    assert!(pool.try_alloc_uninit_rc().is_err());
}

#[test]
fn alloc_error_formatting() {
    // A full bounded pool reports capacity exhaustion, not allocator failure.
    let (pool, _held) = full_pool();
    let err: AllocError = pool.try_alloc_box(99).unwrap_err();
    assert!(err.is_capacity_exhausted());
    assert!(!err.is_allocator_failure());
    assert_eq!(format!("{err:?}"), "AllocError { kind: CapacityExhausted }");
    assert_eq!(format!("{err}"), "the pool reached its maximum capacity");

    // `AllocError` always implements `core::error::Error` (even in `no_std`),
    // so it can be used as a trait object regardless of the `std` feature.
    let as_err: &dyn std::error::Error = &err;
    assert_eq!(as_err.to_string(), "the pool reached its maximum capacity");
    assert!(as_err.source().is_none());
}

// ── every panicking allocator arm (covers each `pool_full()` call site) ──

macro_rules! full_panics {
    ($name:ident, $method:ident $(, $arg:expr)?) => {
        #[test]
        #[should_panic(expected = "the pool reached its maximum capacity")]
        fn $name() {
            let (pool, _held) = full_pool();
            let _ = pool.$method($($arg)?);
        }
    };
}

full_panics!(panic_alloc_box, alloc_box, 1);
full_panics!(panic_alloc_arc, alloc_arc, 1);
full_panics!(panic_alloc, alloc, 1);
full_panics!(panic_alloc_rc, alloc_rc, 1);
full_panics!(panic_alloc_box_with, alloc_box_with, || 1);
full_panics!(panic_alloc_arc_with, alloc_arc_with, || 1);
full_panics!(panic_alloc_with, alloc_with, || 1);
full_panics!(panic_alloc_rc_with, alloc_rc_with, || 1);
full_panics!(panic_alloc_uninit_box, alloc_uninit_box);
full_panics!(panic_alloc_uninit_arc, alloc_uninit_arc);
full_panics!(panic_alloc_uninit, alloc_uninit);
full_panics!(panic_alloc_uninit_rc, alloc_uninit_rc);

// ── builder validation panics ────────────────────────────────────────────

#[test]
#[should_panic(expected = "chunk_size must be >= 1")]
fn chunk_size_zero_panics() {
    let _ = Pool::<u32>::builder().chunk_size(0).build();
}

#[test]
#[should_panic(expected = "chunk_size must be <= 2^31")]
fn chunk_size_too_large_panics() {
    let _ = Pool::<u32>::builder().chunk_size((1 << 31) + 1).build();
}

#[test]
#[should_panic(expected = "exceeds the addressable slot/refcount ceiling")]
fn capacity_overflow_panics() {
    let _ = Pool::<u32>::builder().chunk_size(1 << 16).max_chunks(1 << 16).build();
}

// ── custom allocators ────────────────────────────────────────────────────

// A custom allocator that always fails — exercises `grow`'s allocator-error arm.
struct FailingAllocator;

// SAFETY: `allocate` always returns `Err`, so no memory is ever handed out and
// `deallocate` is never called with a pointer from this allocator.
unsafe impl allocator_api2::alloc::Allocator for FailingAllocator {
    fn allocate(&self, _layout: core::alloc::Layout) -> Result<core::ptr::NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        Err(allocator_api2::alloc::AllocError)
    }
    unsafe fn deallocate(&self, _ptr: core::ptr::NonNull<u8>, _layout: core::alloc::Layout) {}
}

#[test]
fn allocator_failure_surfaces_as_allocator_failure() {
    let pool = Pool::<u32>::builder().chunk_size(4).allocator(FailingAllocator).build();
    // The first allocation must grow a chunk; the allocator fails, so the error
    // identifies the backing allocator as the cause, not a capacity limit.
    let err = pool.try_alloc_box(1).unwrap_err();
    assert!(err.is_allocator_failure());
    assert!(!err.is_capacity_exhausted());
    assert_eq!(format!("{err}"), "the backing allocator failed to allocate a new chunk");
}

// An allocator that tracks the number of live bytes, to prove memory is freed.
#[derive(Clone)]
struct CountingAllocator(std::sync::Arc<std::sync::atomic::AtomicUsize>);

// SAFETY: forwards to `Global` and only adjusts a counter by the same `layout`.
unsafe impl allocator_api2::alloc::Allocator for CountingAllocator {
    fn allocate(&self, layout: core::alloc::Layout) -> Result<core::ptr::NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        let p = Global.allocate(layout)?;
        self.0.fetch_add(layout.size(), std::sync::atomic::Ordering::SeqCst);
        Ok(p)
    }
    unsafe fn deallocate(&self, ptr: core::ptr::NonNull<u8>, layout: core::alloc::Layout) {
        // SAFETY: same contract as `Global::deallocate`.
        unsafe { Global.deallocate(ptr, layout) };
        self.0.fetch_sub(layout.size(), std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn pool_drop_frees_chunks() {
    use std::sync::atomic::Ordering::SeqCst;
    let live = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let pool = Pool::<u64>::builder()
            .chunk_size(8)
            .allocator(CountingAllocator(live.clone()))
            .build();
        let a = pool.alloc_box(1);
        let b = pool.alloc_box(2);
        assert!(live.load(SeqCst) > 0, "a chunk must have been allocated");
        drop((a, b));
    } // dropping the pool must tear it down and free the chunk
    assert_eq!(live.load(SeqCst), 0, "Pool::drop must free every chunk");
}

// ── teardown & concurrency ───────────────────────────────────────────────

#[test]
fn drops_run_exactly_once() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(4).build();
    {
        let mut held = Vec::new();
        for _ in 0..10 {
            held.push(pool.alloc_box(DropCounter(counter.clone())));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

#[test]
fn handles_outlive_the_pool() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(2).build();
    let a = pool.alloc_arc(DropCounter(counter.clone()));
    let b = pool.alloc_box(DropCounter(counter.clone()));
    drop(pool); // pool handle gone, but a and b keep the backing alive
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(a);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    drop(b);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn send_pool_to_another_thread() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();
    let b = pool.alloc_box(123);
    let join = std::thread::spawn(move || {
        let b2 = pool.alloc_box(456);
        *b2 + pool.len() as u32
    });
    assert_eq!(*b, 123);
    let r = join.join().unwrap();
    assert_eq!(r, 456 + 2);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "throughput stress test is too slow under Miri; the concurrent free path is covered by loom"
)]
fn concurrent_frees() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::builder().chunk_size(64).build();

    const N: usize = 4000;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        handles.push(pool.alloc_arc(DropCounter(counter.clone())));
    }
    assert_eq!(pool.len(), N as u64);

    // Spread the handles across threads and drop them concurrently.
    let chunks: Vec<Vec<_>> = {
        let mut iter = handles.into_iter();
        (0..8).map(|_| (0..N / 8).filter_map(|_| iter.next()).collect()).collect()
    };
    std::thread::scope(|s| {
        for chunk in chunks {
            s.spawn(move || {
                for h in chunk {
                    drop(h);
                }
            });
        }
    });

    assert_eq!(counter.load(Ordering::SeqCst), N);
    assert_eq!(pool.len(), 0);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "550k-allocation stress test is too slow under Miri; the CAS-retry paths are covered by loom"
)]
fn contended_free_list_exercises_cas_retries() {
    use std::thread;

    // chunk_size = 1 makes the main thread grow a chunk on essentially every
    // allocation, so the grow-splice CAS loop runs constantly; meanwhile worker
    // threads free a large batch, contending on the free-list head so both the
    // pop loop and the grow-splice loop retry.
    let pool = Pool::<u64>::builder().chunk_size(1).build();

    let batch: Vec<_> = (0..150_000u64).map(|i| pool.alloc_arc(i)).collect();
    let nthreads = 6;
    let mut parts: Vec<Vec<_>> = (0..nthreads).map(|_| Vec::new()).collect();
    for (i, a) in batch.into_iter().enumerate() {
        parts[i % nthreads].push(a);
    }

    thread::scope(|s| {
        for part in parts {
            s.spawn(move || {
                for a in part {
                    drop(a);
                }
            });
        }
        // Force continuous growth while the workers push freed slots.
        let mut held = Vec::with_capacity(400_000);
        for i in 0..400_000u64 {
            held.push(pool.alloc_box(i));
        }
    });

    assert!(pool.chunks_allocated() >= 1);
}
