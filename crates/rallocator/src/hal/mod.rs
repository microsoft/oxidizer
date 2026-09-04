// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(miri)]
mod miri;

#[cfg(not(miri))]
mod native;

#[cfg(all(not(miri), target_os = "windows"))]
mod win64;

#[cfg(all(not(miri), target_os = "linux"))]
mod linux;

#[cfg(all(not(miri), target_os = "linux"))]
use linux as platform;
#[cfg(miri)]
pub(crate) use miri::{
    MEDIUM_MAX_SLICES, MEDIUM_REGION_SIZE, align_down, allocation_prefix_for_read, allocation_prefix_for_write, capture_stack, commit,
    commit_locality_segment, commit_locality_slab, decommit, initialize_storage, map, monotonic_millis, peek_free_requested,
    read_free_next, read_free_requested, release_free_metadata, release_storage, reserve, unmap, write_free_next, write_free_requested,
};
#[cfg(not(miri))]
pub(crate) use native::{
    MEDIUM_MAX_SLICES, MEDIUM_REGION_SIZE, allocation_prefix_for_write, initialize_storage, read_free_next, read_free_requested,
    release_free_metadata, release_storage, write_free_next, write_free_requested,
};
#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) use platform::{monotonic_millis, unmap};
#[cfg(all(not(miri), target_os = "windows"))]
use win64 as platform;

#[cfg(all(test, not(miri)))]
mod faults {
    use std::cell::Cell;

    pub(super) const MAP: u32 = 1 << 0;
    pub(super) const RESERVE: u32 = 1 << 1;
    pub(super) const COMMIT: u32 = 1 << 2;
    pub(super) const COMMIT_LOCALITY_SEGMENT: u32 = 1 << 3;
    pub(super) const COMMIT_LOCALITY_SLAB: u32 = 1 << 4;
    pub(super) const DECOMMIT: u32 = 1 << 5;
    pub(super) const ALIGN_OFFSET: u32 = 1 << 6;
    pub(super) const COMMIT_LOCALITY_SLAB_ZERO: u32 = 1 << 7;
    pub(super) const COMMIT_LOCALITY_SLAB_FULL: u32 = 1 << 8;

    thread_local! {
        static NEXT: Cell<u32> = const { Cell::new(0) };
    }

    pub(super) fn fail_next(operation: u32) {
        NEXT.set(NEXT.get() | operation);
    }

    pub(super) fn take(operation: u32) -> bool {
        NEXT.get() & operation != 0 && {
            NEXT.set(NEXT.get() & !operation);
            true
        }
    }
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) fn map(size: usize) -> *mut u8 {
    #[cfg(test)]
    if faults::take(faults::MAP) {
        return std::ptr::null_mut();
    }
    platform::map(size)
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) fn reserve(size: usize) -> *mut u8 {
    #[cfg(test)]
    if faults::take(faults::RESERVE) {
        return std::ptr::null_mut();
    }
    platform::reserve(size)
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    #[cfg(test)]
    if faults::take(faults::COMMIT) {
        return false;
    }
    unsafe { platform::commit(address, size) }
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) unsafe fn commit_locality_segment(address: *mut u8, segment_size: usize, slab_size: usize) -> Option<usize> {
    #[cfg(test)]
    if faults::take(faults::COMMIT_LOCALITY_SEGMENT) {
        return None;
    }
    unsafe { platform::commit_locality_segment(address, segment_size, slab_size) }
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) unsafe fn commit_locality_slab(address: *mut u8, slab_size: usize) -> Option<usize> {
    #[cfg(test)]
    if faults::take(faults::COMMIT_LOCALITY_SLAB) {
        return None;
    }
    #[cfg(test)]
    if faults::take(faults::COMMIT_LOCALITY_SLAB_ZERO) {
        return Some(0);
    }
    #[cfg(test)]
    if faults::take(faults::COMMIT_LOCALITY_SLAB_FULL) {
        return Some(slab_size);
    }
    unsafe { platform::commit_locality_slab(address, slab_size) }
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    #[cfg(test)]
    if faults::take(faults::DECOMMIT) {
        return false;
    }
    unsafe { platform::decommit(address, size) }
}

#[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
pub(crate) fn align_offset(address: *mut u8, alignment: usize) -> usize {
    #[cfg(test)]
    if faults::take(faults::ALIGN_OFFSET) {
        return usize::MAX;
    }
    address.align_offset(alignment)
}

#[cfg(miri)]
pub(crate) fn align_offset(address: *mut u8, alignment: usize) -> usize {
    address.align_offset(alignment)
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_map() {
    faults::fail_next(faults::MAP);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_reserve() {
    faults::fail_next(faults::RESERVE);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_commit() {
    faults::fail_next(faults::COMMIT);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_commit_locality_segment() {
    faults::fail_next(faults::COMMIT_LOCALITY_SEGMENT);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_commit_locality_slab() {
    faults::fail_next(faults::COMMIT_LOCALITY_SLAB);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn zero_next_commit_locality_slab() {
    faults::fail_next(faults::COMMIT_LOCALITY_SLAB_ZERO);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fully_commit_next_locality_slab() {
    faults::fail_next(faults::COMMIT_LOCALITY_SLAB_FULL);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_decommit() {
    faults::fail_next(faults::DECOMMIT);
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fail_next_align_offset() {
    faults::fail_next(faults::ALIGN_OFFSET);
}

#[cfg(all(not(miri), not(any(target_os = "windows", target_os = "linux"))))]
compile_error!("rallocator currently supports only Windows and Linux");
