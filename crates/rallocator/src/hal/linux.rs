// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::OnceLock;
use std::{process, ptr};

use libc::{
    _SC_PAGESIZE, CLOCK_MONOTONIC, MADV_DONTNEED, MADV_HUGEPAGE, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE,
    clock_gettime, madvise, mmap, mprotect, munmap, sysconf, timespec,
};

const ALLOCATION_ALIGNMENT: usize = 2 * 1024 * 1024;

pub(crate) fn map(size: usize) -> *mut u8 {
    map_aligned(size, PROT_READ | PROT_WRITE)
}

pub(crate) fn reserve(size: usize) -> *mut u8 {
    let address = map_aligned(size, PROT_NONE);
    if address.is_null() {
        return address;
    }
    let _ = unsafe { madvise(address.cast(), size, MADV_HUGEPAGE) };
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
    unsafe { mprotect(address.cast(), size, PROT_NONE) == 0 && madvise(address.cast(), size, MADV_DONTNEED) == 0 }
}

pub(crate) unsafe fn unmap(address: *mut u8, size: usize) {
    // Linux accepts an unaligned length and unmaps every page intersecting the range.
    let released = unsafe { munmap(address.cast(), size) };
    abort_on_failure(released);
}

pub(crate) fn monotonic_millis() -> u64 {
    let mut time = timespec { tv_sec: 0, tv_nsec: 0 };
    let result = unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut time) };
    abort_on_failure(result);
    (time.tv_sec as u64)
        .saturating_mul(1_000)
        .saturating_add(time.tv_nsec as u64 / 1_000_000)
}

fn map_aligned(size: usize, protection: i32) -> *mut u8 {
    map_aligned_with_page_size(size, protection, page_size())
}

fn map_aligned_with_page_size(size: usize, protection: i32, page_size: usize) -> *mut u8 {
    let Some(rounded_size) = size.checked_add(page_size - 1).map(|size| size & !(page_size - 1)) else {
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
        abort_on_failure(result);
    }

    let suffix = unsafe { aligned.add(rounded_size) };
    let result = unsafe { munmap(suffix.cast(), suffix_size) };
    abort_on_failure(result);
    aligned
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn abort_on_failure(result: i32) {
    if result != 0 {
        process::abort();
    }
}

fn page_size() -> usize {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();

    *PAGE_SIZE.get_or_init(|| {
        let raw_page_size = unsafe { sysconf(_SC_PAGESIZE) };
        validated_page_size(raw_page_size).unwrap_or_else(abort_invalid_page_size)
    })
}

fn validated_page_size(raw_page_size: libc::c_long) -> Option<usize> {
    let page_size = usize::try_from(raw_page_size).ok()?;
    (page_size != 0 && page_size.is_power_of_two() && ALLOCATION_ALIGNMENT.is_multiple_of(page_size)).then_some(page_size)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn abort_invalid_page_size<T>() -> T {
    process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_capture_clamps_to_its_fixed_buffer() {
        let mut frames = [0; 128];
        assert!(capture_stack(&mut frames, usize::MAX) <= 60);
    }

    #[test]
    fn host_page_size_is_compatible_with_allocator_alignment() {
        let page_size = page_size();
        assert!(page_size.is_power_of_two());
        assert!(ALLOCATION_ALIGNMENT.is_multiple_of(page_size));
    }

    #[test]
    fn aligned_mapping_works_with_large_page_granularities() {
        let host_page_size = page_size();
        // Simulated granularities exercise rounding and alignment independently
        // of the host. The real syscalls still run against host pages, including
        // unmapping the deliberately unrounded logical size below.
        for simulated_page_size in [16_usize * 1024, 64 * 1024]
            .into_iter()
            .filter(|page_size| (*page_size).is_multiple_of(host_page_size))
        {
            let size = simulated_page_size + 1;
            let address = map_aligned_with_page_size(size, PROT_READ | PROT_WRITE, simulated_page_size);
            assert!(!address.is_null());
            assert!(address.addr().is_multiple_of(ALLOCATION_ALIGNMENT));
            unsafe { unmap(address, size) };
        }
    }

    #[test]
    fn mapping_failures_and_page_size_validation_are_reported() {
        let host_page_size = page_size();
        assert!(map_aligned_with_page_size(usize::MAX, PROT_READ | PROT_WRITE, host_page_size).is_null());
        assert!(map_aligned_with_page_size(usize::MAX - (host_page_size - 1), PROT_READ | PROT_WRITE, host_page_size).is_null());
        assert!(map_aligned_with_page_size(isize::MAX as usize, PROT_READ | PROT_WRITE, host_page_size).is_null());
        assert!(reserve(usize::MAX).is_null());

        assert_eq!(
            validated_page_size(libc::c_long::try_from(host_page_size).unwrap()),
            Some(host_page_size)
        );
        assert_eq!(validated_page_size(-1), None);
        assert_eq!(validated_page_size(0), None);
        assert_eq!(validated_page_size(3), None);
        assert_eq!(validated_page_size(libc::c_long::try_from(ALLOCATION_ALIGNMENT * 2).unwrap()), None);

        assert!(!unsafe { decommit(ptr::without_provenance_mut(1), host_page_size) });
    }
}
