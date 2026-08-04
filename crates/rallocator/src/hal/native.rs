use std::mem::size_of;

pub(crate) const MEDIUM_MAX_SLICES: usize = 512;
pub(crate) const MEDIUM_REGION_SIZE: usize = 1024 * 1024 * 1024;

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
