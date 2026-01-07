// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(all(not(miri), target_os = "linux"))]
#[cfg_attr(coverage_nightly, coverage(off))] // Test utility function, not meant to be covered
pub(crate) fn system_memory() -> usize {
    use std::mem::MaybeUninit;

    let mut sys_info: MaybeUninit<libc::sysinfo> = MaybeUninit::uninit();

    // SAFETY: Call sysinfo syscall with a valid pointer.
    let return_code = unsafe { libc::sysinfo(sys_info.as_mut_ptr()) };

    assert!(return_code == 0, "sysinfo syscall failed with return code {return_code}");

    // SAFETY: sysinfo syscall initialized the structure.
    let sys_info = unsafe { sys_info.assume_init() };

    usize::try_from(sys_info.totalram).expect("total memory exceeds usize")
}

#[cfg(all(not(miri), target_os = "windows"))]
#[cfg_attr(coverage_nightly, coverage(off))] // Test utility function, not meant to be covered
pub(crate) fn system_memory() -> usize {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut mem_status_ex = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).expect("MEMORYSTATUSEX size exceeds u32"),
        ..Default::default()
    };

    // SAFETY: GlobalMemoryStatusEx syscall with a valid pointer.
    let return_value = unsafe { GlobalMemoryStatusEx(&raw mut mem_status_ex) };

    if return_value == 0 {
        use windows_sys::Win32::Foundation::GetLastError;

        // SAFETY: GetLastError is always safe to call.
        let error = unsafe { GetLastError() };
        panic!("GlobalMemoryStatusEx syscall failed: {error}");
    } else {
        usize::try_from(mem_status_ex.ullTotalPhys).expect("total memory exceeds usize")
    }
}
