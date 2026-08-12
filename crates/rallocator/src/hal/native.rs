// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) const MEDIUM_MAX_SLICES: usize = 512;
pub(crate) const MEDIUM_REGION_SIZE: usize = 1024 * 1024 * 1024;

#[inline(always)]
pub(crate) unsafe fn allocation_prefix_for_write<T>(address: *mut u8, offset: usize) -> *mut T {
    unsafe { address.sub(offset).cast() }
}

pub(crate) unsafe fn initialize_storage<T>(embedded: *mut T, value: T) -> *mut T {
    unsafe { embedded.write(value) };
    embedded
}

pub(crate) unsafe fn release_storage<T>(storage: *mut T, embedded: *mut T) {
    debug_assert_eq!(storage, embedded);
}

#[inline(always)]
pub(crate) unsafe fn write_free_next(block: *mut u8, next: *mut u8) {
    unsafe { block.cast::<*mut u8>().write(next) };
}

#[inline(always)]
pub(crate) unsafe fn read_free_next(block: *mut u8) -> *mut u8 {
    unsafe { block.cast::<*mut u8>().read() }
}

#[inline(always)]
pub(crate) unsafe fn write_free_requested(block: *mut u8, requested_bytes: usize) {
    unsafe { block.add(size_of::<usize>()).cast::<usize>().write(requested_bytes) };
}

#[inline(always)]
pub(crate) unsafe fn read_free_requested(block: *mut u8) -> usize {
    unsafe { block.add(size_of::<usize>()).cast::<usize>().read() }
}

#[inline(always)]
pub(crate) unsafe fn release_free_metadata(_block: *mut u8) {}

#[inline(always)]
pub(crate) unsafe fn peek_free_requested(block: *mut u8) -> usize {
    unsafe { read_free_requested(block) }
}
