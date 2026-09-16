// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation and CPU measurements for the opt-in large-file regression test.

#![cfg_attr(coverage_nightly, coverage(off))] // Manual profiling support, not product behavior.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: AllocationCounter = AllocationCounter;

struct AllocationCounter;

// SAFETY: every operation delegates the unchanged allocation contract to System.
unsafe impl GlobalAlloc for AllocationCounter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies a valid layout, forwarded unchanged to System.
        let pointer = unsafe { System.alloc(layout) };
        record_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies a valid layout, forwarded unchanged to System.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if ENABLED.load(Ordering::Relaxed) {
            DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: this allocation came from System, and the caller supplies its original layout.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller supplies a System allocation, its layout, and a valid new size.
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() && ENABLED.load(Ordering::Relaxed) {
            DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            record_allocation(result, new_size);
        }
        result
    }
}

fn record_allocation(pointer: *mut u8, bytes: usize) {
    if !pointer.is_null() && ENABLED.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

pub(super) struct Sample {
    allocations: u64,
    deallocations: u64,
    allocated_bytes: u64,
    cpu_seconds: f64,
}

pub(super) fn enable_allocations() {
    ENABLED.store(true, Ordering::Relaxed);
}

impl Sample {
    pub(super) fn now() -> Self {
        Self {
            allocations: ALLOCATIONS.load(Ordering::Relaxed),
            deallocations: DEALLOCATIONS.load(Ordering::Relaxed),
            allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
            cpu_seconds: cpu_seconds(),
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "human-readable profiling sizes do not require byte-exact floating point"
    )]
    pub(super) fn describe_since(&self, before: &Self) -> String {
        if !ENABLED.load(Ordering::Relaxed) {
            return format!("CPU {:.3}s; allocation counting disabled", self.cpu_seconds - before.cpu_seconds);
        }
        format!(
            "CPU {:.3}s, {} allocations, {} frees, {:.3} GiB allocated",
            self.cpu_seconds - before.cpu_seconds,
            self.allocations - before.allocations,
            self.deallocations - before.deallocations,
            (self.allocated_bytes - before.allocated_bytes) as f64 / 1_073_741_824.0,
        )
    }
}

#[cfg(windows)]
#[expect(
    clippy::cast_precision_loss,
    reason = "human-readable CPU seconds do not require tick-exact floating point"
)]
fn cpu_seconds() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    let (mut creation, mut exit, mut kernel, mut user) =
        (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: obtaining the current process pseudo-handle has no preconditions.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: the pseudo-handle names this process; all four outputs point to live,
    // correctly aligned FILETIME storage for the duration of the synchronous call.
    let success = unsafe { GetProcessTimes(process, &raw mut creation, &raw mut exit, &raw mut kernel, &raw mut user) };
    assert_ne!(success, 0, "the current process must be queryable by its pseudo-handle");
    let ticks = |time: FileTime| (u64::from(time.high) << 32) | u64::from(time.low);
    (ticks(kernel) + ticks(user)) as f64 / 10_000_000.0
}

#[cfg(not(windows))]
fn cpu_seconds() -> f64 {
    f64::NAN
}
