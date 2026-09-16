// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ptr;

use windows_sys::Win32::System::Kernel::PROCESSOR_NUMBER;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub(crate) fn memory_status() -> Option<super::MemoryStatus> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: the OS writes the correctly sized, stack-resident structure.
    if unsafe { GlobalMemoryStatusEx(&raw mut status) } == 0 {
        return None;
    }
    Some(super::MemoryStatus {
        total: status.ullTotalPhys.min(status.ullTotalPageFile) as usize,
        available: status.ullAvailPhys.min(status.ullAvailPageFile) as usize,
    })
}
use windows_sys::Win32::System::Threading::{GetCurrentProcessorNumberEx, GetNumaProcessorNodeEx};

pub(crate) fn current_processor_location() -> (usize, usize) {
    let mut processor = PROCESSOR_NUMBER {
        Group: 0,
        Number: 0,
        Reserved: 0,
    };
    let mut node = 0_u16;
    // SAFETY: these allocation-free OS queries write only the stack outputs.
    unsafe {
        GetCurrentProcessorNumberEx(&raw mut processor);
        if GetNumaProcessorNodeEx(&raw const processor, &raw mut node) == 0 || node == u16::MAX {
            node = 0;
        }
    }
    (usize::from(processor.Group) * 64 + usize::from(processor.Number), usize::from(node))
}

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
