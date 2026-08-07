// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ptr;

use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

pub(crate) fn map(size: usize) -> *mut u8 {
    unsafe { VirtualAlloc(ptr::null_mut(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE).cast() }
}

pub(crate) fn reserve(size: usize) -> *mut u8 {
    unsafe { VirtualAlloc(ptr::null_mut(), size, MEM_RESERVE, PAGE_READWRITE).cast() }
}

pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    !unsafe { VirtualAlloc(address.cast(), size, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

pub(crate) unsafe fn commit_locality_segment(address: *mut u8, _segment_size: usize, slab_size: usize) -> Option<usize> {
    unsafe { commit(address, slab_size) }.then_some(slab_size)
}

pub(crate) unsafe fn commit_locality_slab(address: *mut u8, slab_size: usize) -> Option<usize> {
    unsafe { commit(address, slab_size) }.then_some(slab_size)
}

pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    (unsafe { VirtualFree(address.cast(), size, MEM_DECOMMIT) }) != 0
}

pub(crate) unsafe fn unmap(address: *mut u8, _size: usize) {
    let released = unsafe { VirtualFree(address.cast(), 0, MEM_RELEASE) };
    debug_assert_ne!(released, 0);
}

pub(crate) fn monotonic_millis() -> u64 {
    unsafe { GetTickCount64() }
}

pub(crate) fn capture_stack(frames: &mut [usize], limit: usize) -> usize {
    let limit = limit.min(frames.len()).min(u32::MAX as usize);
    if limit == 0 {
        return 0;
    }
    unsafe { RtlCaptureStackBackTrace(4, limit as u32, frames.as_mut_ptr().cast(), ptr::null_mut()) as usize }
}
