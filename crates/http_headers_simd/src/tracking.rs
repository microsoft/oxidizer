// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-wide allocation accounting for the workspace performance scripts.
//!
//! This module exists so allocation claims in `docs/PERF.md` can be measured
//! without any project-authored `unsafe` outside this crate. It is compiled
//! only under the `benchmarking` feature and is never part of a normal build.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[repr(align(64))]
struct Counters {
    locked: AtomicBool,
    allocations: AtomicU64,
    allocated_bytes: AtomicU64,
    live_bytes: AtomicUsize,
    peak_live_bytes: AtomicUsize,
}

static COUNTERS: Counters = Counters {
    locked: AtomicBool::new(false),
    allocations: AtomicU64::new(0),
    allocated_bytes: AtomicU64::new(0),
    live_bytes: AtomicUsize::new(0),
    peak_live_bytes: AtomicUsize::new(0),
};

struct CounterGuard;

impl Drop for CounterGuard {
    fn drop(&mut self) {
        COUNTERS.locked.store(false, Ordering::Release);
    }
}

fn lock_counters() -> CounterGuard {
    lock_counters_with_attempt_hook(|_| {})
}

fn lock_counters_with_attempt_hook(mut after_attempt: impl FnMut(bool)) -> CounterGuard {
    loop {
        let acquired = COUNTERS
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok();
        after_attempt(acquired);
        if acquired {
            return CounterGuard;
        }
        hint::spin_loop();
    }
}

/// A snapshot of the counters maintained by [`TrackingAllocator`].
///
/// Fields are public so benchmark binaries can consume snapshots as passive
/// data without accessor overhead.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "benchmarking", feature = "std"))]
/// # {
/// let stats = http_headers_simd::tracking::AllocationStats::default();
/// assert_eq!(stats.allocations, 0);
/// assert_eq!(stats.live_bytes, 0);
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationStats {
    /// The number of successful allocations, including growing reallocations.
    pub allocations: u64,
    /// The total number of bytes handed out, including reallocation growth.
    pub allocated_bytes: u64,
    /// The number of bytes currently allocated and not yet released.
    pub live_bytes: usize,
    /// The largest observed value of [`AllocationStats::live_bytes`].
    pub peak_live_bytes: usize,
}

/// A [`GlobalAlloc`] that forwards to the system allocator and counts requests.
///
/// Install it in a measurement binary with the `#[global_allocator]` attribute.
/// Counting is process-wide. Counter updates and snapshots are serialized, so
/// concurrent allocations are accounted coherently.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(all(feature = "benchmarking", feature = "std"))]
/// # {
/// use http_headers_simd::tracking::TrackingAllocator;
///
/// #[global_allocator]
/// static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackingAllocator;

impl TrackingAllocator {
    /// Creates the allocator.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "benchmarking", feature = "std"))]
    /// # {
    /// let allocator = http_headers_simd::tracking::TrackingAllocator::new();
    /// let _ = allocator;
    /// # }
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

// SAFETY: every method forwards its arguments unchanged to `System`, which is
// a correct `GlobalAlloc`, and returns exactly what `System` returned. The
// counter updates neither allocate nor touch the returned memory, so all of
// `GlobalAlloc`'s pointer, layout, and aliasing obligations are discharged by
// `System` itself.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unchanged from the caller, which the
        // `GlobalAlloc` contract already requires to be valid for `System`.
        let pointer = unsafe { System.alloc(layout) };
        record_allocation(pointer, layout.size())
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unchanged from the caller, which the
        // `GlobalAlloc` contract already requires to be valid for `System`.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record_allocation(pointer, layout.size())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        {
            let _guard = lock_counters();
            let _ = COUNTERS.live_bytes.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(layout.size()))
            });
        }
        // SAFETY: `ptr` and `layout` are forwarded unchanged from the
        // caller, which the `GlobalAlloc` contract already requires to name a
        // block this allocator returned for that layout.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: all three arguments are forwarded unchanged from the caller,
        // which the `GlobalAlloc` contract already requires to be valid.
        let replacement = unsafe { System.realloc(ptr, layout, new_size) };
        record_reallocation(replacement, layout.size(), new_size)
    }
}

#[inline]
fn record_allocation(pointer: *mut u8, size: usize) -> *mut u8 {
    if !pointer.is_null() {
        record(size);
    }
    pointer
}

#[inline]
fn record_reallocation(replacement: *mut u8, old_size: usize, new_size: usize) -> *mut u8 {
    if replacement.is_null() {
        return replacement;
    }
    if let Some(growth) = new_size.checked_sub(old_size) {
        record(growth);
    } else {
        let shrink = old_size.saturating_sub(new_size);
        let _guard = lock_counters();
        let _ = COUNTERS
            .live_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| Some(live.saturating_sub(shrink)));
    }
    replacement
}

fn record(bytes: usize) {
    let _guard = lock_counters();
    let _ = COUNTERS.allocations.fetch_add(1, Ordering::Relaxed);
    let _ = COUNTERS.allocated_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    let live = COUNTERS.live_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let _ = COUNTERS.peak_live_bytes.fetch_max(live, Ordering::Relaxed);
}

/// Reads the current counters.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "benchmarking", feature = "std"))]
/// # {
/// let stats = http_headers_simd::tracking::allocation_stats();
/// assert!(stats.peak_live_bytes >= stats.live_bytes);
/// # }
/// ```
#[must_use]
pub fn allocation_stats() -> AllocationStats {
    let _guard = lock_counters();
    AllocationStats {
        allocations: COUNTERS.allocations.load(Ordering::Relaxed),
        allocated_bytes: COUNTERS.allocated_bytes.load(Ordering::Relaxed),
        live_bytes: COUNTERS.live_bytes.load(Ordering::Relaxed),
        peak_live_bytes: COUNTERS.peak_live_bytes.load(Ordering::Relaxed),
    }
}

/// Resets the peak watermark to the currently live byte count.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "benchmarking", feature = "std"))]
/// # {
/// use http_headers_simd::tracking::{allocation_stats, reset_peak_live_bytes};
///
/// reset_peak_live_bytes();
/// let stats = allocation_stats();
/// assert_eq!(stats.peak_live_bytes, stats.live_bytes);
/// # }
/// ```
pub fn reset_peak_live_bytes() {
    let _guard = lock_counters();
    COUNTERS
        .peak_live_bytes
        .store(COUNTERS.live_bytes.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Returns the counter deltas accumulated between two snapshots.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "benchmarking", feature = "std"))]
/// # {
/// use http_headers_simd::tracking::{AllocationStats, allocation_delta};
///
/// let before = AllocationStats {
///     allocations: 2,
///     allocated_bytes: 32,
///     live_bytes: 8,
///     peak_live_bytes: 16,
/// };
/// let after = AllocationStats {
///     allocations: 5,
///     allocated_bytes: 80,
///     live_bytes: 24,
///     peak_live_bytes: 40,
/// };
/// let delta = allocation_delta(before, after);
/// assert_eq!(delta.allocations, 3);
/// assert_eq!(delta.allocated_bytes, 48);
/// assert_eq!(delta.live_bytes, 16);
/// assert_eq!(delta.peak_live_bytes, 32);
/// # }
/// ```
#[must_use]
pub fn allocation_delta(before: AllocationStats, after: AllocationStats) -> AllocationStats {
    AllocationStats {
        allocations: after.allocations.saturating_sub(before.allocations),
        allocated_bytes: after.allocated_bytes.saturating_sub(before.allocated_bytes),
        live_bytes: after.live_bytes.saturating_sub(before.live_bytes),
        peak_live_bytes: after.peak_live_bytes.saturating_sub(before.live_bytes),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::mpsc::{self, TryRecvError};
    use std::sync::{Mutex, PoisonError};
    use std::{ptr, slice, thread};

    use super::{
        AllocationStats, TrackingAllocator, allocation_delta, allocation_stats, lock_counters, lock_counters_with_attempt_hook, record,
        record_allocation, record_reallocation, reset_peak_live_bytes,
    };

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn concurrent_updates_produce_a_coherent_snapshot() {
        const THREADS: usize = 4;
        const UPDATES: usize = if cfg!(miri) { 32 } else { 1_000 };
        let _serial = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);

        let before = allocation_stats();
        thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    for _ in 0..UPDATES {
                        record(1);
                    }
                });
            }
        });
        let delta = allocation_delta(before, allocation_stats());

        assert_eq!(delta.allocations, (THREADS * UPDATES) as u64);
        assert_eq!(delta.allocated_bytes, (THREADS * UPDATES) as u64);
        assert_eq!(delta.live_bytes, THREADS * UPDATES);
    }

    #[test]
    fn global_alloc_methods_preserve_memory_and_counters() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let allocator = TrackingAllocator::new();
        let layout = Layout::from_size_align(32, 8).expect("valid test layout");
        reset_peak_live_bytes();
        let before = allocation_stats();

        // SAFETY: `layout` is valid and the returned pointer is checked before use.
        let pointer = unsafe { allocator.alloc_zeroed(layout) };
        assert!(!pointer.is_null());
        assert!(
            // SAFETY: the allocation above contains 32 initialized bytes.
            unsafe { slice::from_raw_parts(pointer, 32) }.iter().all(|byte| *byte == 0)
        );

        // SAFETY: `pointer` and `layout` identify the live allocation above.
        let pointer = unsafe { allocator.realloc(pointer, layout, 64) };
        assert!(!pointer.is_null());
        let grown = Layout::from_size_align(64, 8).expect("valid grown layout");
        // SAFETY: `pointer` and `grown` identify the live reallocated block.
        unsafe { allocator.dealloc(pointer, grown) };

        let delta = allocation_delta(before, allocation_stats());
        assert_eq!(delta.allocations, 2);
        assert_eq!(delta.allocated_bytes, 64);
        assert_eq!(delta.live_bytes, 0);
        assert_eq!(delta.peak_live_bytes, 64);
    }

    #[test]
    fn allocating_and_shrinking_preserve_memory_and_counters() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let allocator = TrackingAllocator;
        let layout = Layout::from_size_align(64, 8).expect("valid test layout");
        reset_peak_live_bytes();
        let before = allocation_stats();

        // SAFETY: `layout` is valid and the returned pointer is checked before use.
        let pointer = unsafe { allocator.alloc(layout) };
        assert!(!pointer.is_null());
        // SAFETY: the allocation above contains 64 writable bytes.
        unsafe { pointer.write_bytes(0x5a, 64) };

        // SAFETY: `pointer` and `layout` identify the live allocation above.
        let pointer = unsafe { allocator.realloc(pointer, layout, 16) };
        assert!(!pointer.is_null());
        assert!(
            // SAFETY: the replacement allocation contains the preserved 16-byte prefix.
            unsafe { slice::from_raw_parts(pointer, 16) }.iter().all(|byte| *byte == 0x5a)
        );
        let shrunk = Layout::from_size_align(16, 8).expect("valid shrunk layout");
        // SAFETY: `pointer` and `shrunk` identify the live reallocated block.
        unsafe { allocator.dealloc(pointer, shrunk) };

        let delta = allocation_delta(before, allocation_stats());
        assert_eq!(delta.allocations, 1);
        assert_eq!(delta.allocated_bytes, 64);
        assert_eq!(delta.live_bytes, 0);
        assert_eq!(delta.peak_live_bytes, 64);
    }

    #[test]
    fn allocation_delta_saturates_independent_counters() {
        let before = AllocationStats {
            allocations: 10,
            allocated_bytes: 20,
            live_bytes: 30,
            peak_live_bytes: 40,
        };
        let after = AllocationStats {
            allocations: 5,
            allocated_bytes: 25,
            live_bytes: 10,
            peak_live_bytes: 35,
        };
        assert_eq!(
            allocation_delta(before, after),
            AllocationStats {
                allocations: 0,
                allocated_bytes: 5,
                live_bytes: 0,
                peak_live_bytes: 5,
            }
        );
    }

    #[test]
    fn failed_allocator_operations_leave_counters_unchanged() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let layout = Layout::from_size_align(32, 8).expect("valid test layout");
        let before = allocation_stats();

        let allocated = record_allocation(ptr::null_mut(), layout.size());
        assert!(allocated.is_null());
        let zeroed = record_allocation(ptr::null_mut(), layout.size());
        assert!(zeroed.is_null());
        let reallocated = record_reallocation(ptr::null_mut(), layout.size(), 64);
        assert!(reallocated.is_null());
        assert_eq!(allocation_stats(), before);
    }

    #[test]
    fn counter_lock_waits_until_the_current_guard_is_released() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let guard = lock_counters();
        let (attempted_send, attempted_receive) = mpsc::sync_channel(0);
        let (resume_send, resume_receive) = mpsc::sync_channel(0);
        let (acquired_send, acquired_receive) = mpsc::sync_channel(0);

        thread::scope(|scope| {
            let waiter = scope.spawn(move || {
                let waiter_guard = lock_counters_with_attempt_hook(|acquired| {
                    attempted_send.send(acquired).unwrap();
                    resume_receive.recv().unwrap();
                });
                acquired_send.send(()).unwrap();
                drop(waiter_guard);
            });

            let acquired_while_locked = attempted_receive.recv().unwrap();
            let completed_while_locked = acquired_receive.try_recv();
            drop(guard);

            // Weak compare-exchange can fail spuriously after the guard is released.
            let mut acquired = acquired_while_locked;
            while !acquired {
                resume_send.send(()).unwrap();
                acquired = attempted_receive.recv().unwrap();
            }
            let completed_before_resume = acquired_receive.try_recv();
            resume_send.send(()).unwrap();
            acquired_receive.recv().unwrap();
            waiter.join().unwrap();

            assert!(!acquired_while_locked);
            assert_eq!(completed_while_locked, Err(TryRecvError::Empty));
            assert_eq!(completed_before_resume, Err(TryRecvError::Empty));
        });
    }
}
