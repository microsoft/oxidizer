// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ptr;

use libc::{
    CLOCK_MONOTONIC, MADV_DONTNEED, MADV_HUGEPAGE, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE, clock_gettime,
    madvise, mmap, mprotect, munmap, timespec,
};

const ALLOCATION_ALIGNMENT: usize = 2 * 1024 * 1024;
const PAGE_SIZE: usize = 4 * 1024;

pub(crate) fn map(size: usize) -> *mut u8 {
    map_aligned(size, PROT_READ | PROT_WRITE)
}

pub(crate) fn reserve(size: usize) -> *mut u8 {
    let address = map_aligned(size, PROT_READ | PROT_WRITE);
    if !address.is_null() {
        let advised = unsafe { madvise(address.cast(), size, MADV_HUGEPAGE) };
        debug_assert_eq!(advised, 0);
    }
    address
}

pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    unsafe { mprotect(address.cast(), size, PROT_READ | PROT_WRITE) == 0 }
}

pub(crate) unsafe fn commit_locality_segment(address: *mut u8, segment_size: usize, _slab_size: usize) -> Option<usize> {
    unsafe { commit(address, segment_size) }.then_some(segment_size)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the platform HAL has one shared fallible locality-commit signature"
)]
pub(crate) unsafe fn commit_locality_slab(_address: *mut u8, _slab_size: usize) -> Option<usize> {
    Some(0)
}

pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    if unsafe { mprotect(address.cast(), size, PROT_NONE) } != 0 {
        return false;
    }
    unsafe { madvise(address.cast(), size, MADV_DONTNEED) == 0 }
}

pub(crate) unsafe fn unmap(address: *mut u8, size: usize) {
    let released = unsafe { munmap(address.cast(), size) };
    debug_assert_eq!(released, 0);
}

pub(crate) fn monotonic_millis() -> u64 {
    let mut time = timespec { tv_sec: 0, tv_nsec: 0 };
    let result = unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut time) };
    debug_assert_eq!(result, 0);
    (time.tv_sec as u64)
        .saturating_mul(1_000)
        .saturating_add(time.tv_nsec as u64 / 1_000_000)
}

pub(crate) fn capture_stack(frames: &mut [usize], limit: usize) -> usize {
    const SKIPPED_FRAMES: usize = 4;
    const MAX_CAPTURED_FRAMES: usize = 64;

    let limit = limit.min(frames.len()).min(MAX_CAPTURED_FRAMES - SKIPPED_FRAMES);
    if limit == 0 {
        return 0;
    }
    let mut captured_frames = [0_usize; MAX_CAPTURED_FRAMES];
    let frame_count = i32::try_from(limit + SKIPPED_FRAMES).expect("frame count is bounded by the 64-entry local array");
    let captured = unsafe { libc::backtrace(captured_frames.as_mut_ptr().cast(), frame_count) }.max(0) as usize;
    let skipped = captured.min(SKIPPED_FRAMES);
    let retained = (captured - skipped).min(limit);
    frames[..retained].copy_from_slice(&captured_frames[skipped..skipped + retained]);
    retained
}

fn map_aligned(size: usize, protection: i32) -> *mut u8 {
    let Some(rounded_size) = size.checked_add(PAGE_SIZE - 1).map(|size| size & !(PAGE_SIZE - 1)) else {
        return ptr::null_mut();
    };
    let Some(mapping_size) = rounded_size.checked_add(ALLOCATION_ALIGNMENT) else {
        return ptr::null_mut();
    };
    let mapping = unsafe { mmap(ptr::null_mut(), mapping_size, protection, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) };
    if mapping == MAP_FAILED {
        return ptr::null_mut();
    }

    let mapping = mapping.cast::<u8>();
    let aligned_address = (mapping.addr() + ALLOCATION_ALIGNMENT - 1) & !(ALLOCATION_ALIGNMENT - 1);
    let aligned = mapping.map_addr(|_| aligned_address);
    let prefix_size = aligned_address - mapping.addr();
    let suffix_size = mapping_size - prefix_size - rounded_size;
    if prefix_size != 0 {
        let result = unsafe { munmap(mapping.cast(), prefix_size) };
        debug_assert_eq!(result, 0);
    }

    if suffix_size != 0 {
        let suffix = unsafe { aligned.add(rounded_size) };
        let result = unsafe { munmap(suffix.cast(), suffix_size) };
        debug_assert_eq!(result, 0);
    }
    aligned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_capture_clamps_to_its_fixed_buffer() {
        crate::initialize();
        let mut frames = [0; 128];
        assert!(capture_stack(&mut frames, usize::MAX) <= 60);
    }
}
