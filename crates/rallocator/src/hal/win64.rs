// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ptr;

use windows_sys::Win32::System::Kernel::PROCESSOR_NUMBER;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};

#[cfg_attr(test, mutants::skip)] // Linux mutation runs enumerate Windows-only source that cannot execute there.
pub(crate) fn memory_status() -> Option<super::MemoryStatus> {
    memory_status_with(|status| {
        // SAFETY: the OS writes the correctly sized, stack-resident structure.
        unsafe { GlobalMemoryStatusEx(status) != 0 }
    })
}

#[cfg_attr(test, mutants::skip)] // Linux mutation runs enumerate Windows-only source that cannot execute there.
fn memory_status_with(query: impl FnOnce(&mut MEMORYSTATUSEX) -> bool) -> Option<super::MemoryStatus> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if !query(&mut status) {
        return None;
    }
    Some(super::MemoryStatus {
        total: status.ullTotalPhys.min(status.ullTotalPageFile) as usize,
        available: status.ullAvailPhys.min(status.ullAvailPageFile) as usize,
    })
}
use windows_sys::Win32::System::Threading::{GetCurrentProcessorNumberEx, GetNumaProcessorNodeEx};

#[cfg_attr(test, mutants::skip)] // Linux mutation runs enumerate Windows-only source that cannot execute there.
pub(crate) fn current_processor_location() -> (usize, usize) {
    processor_location_with(|processor, node| {
        // SAFETY: these allocation-free OS queries write only the stack outputs.
        unsafe {
            GetCurrentProcessorNumberEx(processor);
            GetNumaProcessorNodeEx(processor, node) != 0
        }
    })
}

#[cfg_attr(test, mutants::skip)] // Linux mutation runs enumerate Windows-only source that cannot execute there.
fn processor_location_with(query: impl FnOnce(&mut PROCESSOR_NUMBER, &mut u16) -> bool) -> (usize, usize) {
    let mut processor = PROCESSOR_NUMBER {
        Group: 0,
        Number: 0,
        Reserved: 0,
    };
    let mut node = 0_u16;
    if !query(&mut processor, &mut node) || node == u16::MAX {
        node = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_query_follows_the_calling_threads_affinity() {
        use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadGroupAffinity, SetThreadGroupAffinity};

        std::thread::spawn(|| {
            let mut allowed = GROUP_AFFINITY::default();
            // SAFETY: the pseudo handle identifies this live worker, and the
            // writable affinity output is correctly sized.
            assert_ne!(unsafe { GetThreadGroupAffinity(GetCurrentThread(), &raw mut allowed) }, 0);
            for number in 0..usize::BITS {
                let mask = 1_usize << number;
                if allowed.Mask & mask == 0 {
                    continue;
                }
                let affinity = GROUP_AFFINITY { Mask: mask, ..allowed };
                let mut previous = GROUP_AFFINITY::default();
                // SAFETY: each mask is a nonempty subset of this worker's
                // allowed group. Only this dedicated worker changes affinity;
                // thread exit releases it without affecting the test harness.
                assert_ne!(
                    unsafe { SetThreadGroupAffinity(GetCurrentThread(), &raw const affinity, &raw mut previous) },
                    0
                );
                let processor = PROCESSOR_NUMBER {
                    Group: allowed.Group,
                    Number: number as u8,
                    Reserved: 0,
                };
                let mut node = 0;
                // SAFETY: this is an allowed processor and a live output cell.
                let found = unsafe { GetNumaProcessorNodeEx(&raw const processor, &raw mut node) } != 0;
                let expected_node = if found && node != u16::MAX { usize::from(node) } else { 0 };
                assert_eq!(
                    current_processor_location(),
                    (usize::from(allowed.Group) * 64 + number as usize, expected_node)
                );
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn host_memory_query_reports_nonzero_consistent_bounds() {
        let memory = memory_status().unwrap();
        assert!(memory.total > 0);
        assert!(memory.available <= memory.total);
    }

    #[test]
    fn unavailable_memory_status_does_not_invent_a_budget() {
        assert!(
            memory_status_with(|status| {
                assert_eq!(status.dwLength as usize, size_of::<MEMORYSTATUSEX>());
                false
            })
            .is_none()
        );
    }

    #[test]
    fn memory_budget_uses_the_tighter_physical_and_commit_limits() {
        let status = memory_status_with(|status| {
            status.ullTotalPhys = 100;
            status.ullTotalPageFile = 80;
            status.ullAvailPhys = 20;
            status.ullAvailPageFile = 30;
            true
        })
        .unwrap();
        assert_eq!((status.total, status.available), (80, 20));
    }

    #[test]
    fn processor_location_preserves_group_when_numa_is_unavailable() {
        let locations = [(false, 3), (true, u16::MAX), (true, 3)].map(|(success, reported)| {
            processor_location_with(|processor, node| {
                processor.Group = 2;
                processor.Number = 7;
                *node = reported;
                success
            })
        });
        assert_eq!(locations, [(135, 0), (135, 0), (135, 3)]);
    }
}
