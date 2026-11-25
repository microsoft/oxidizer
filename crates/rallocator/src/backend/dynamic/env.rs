// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Startup lookup must not allocate through Rust's global allocator. In
//! particular, `std::env::var_os` allocates an owned string for a present value.

#[cfg(unix)]
use std::os::fd::RawFd as Output;
#[cfg(windows)]
use std::os::windows::io::RawHandle as Output;

use super::{Choice, Setting};

fn parse<T: Copy + PartialEq + From<u8>>(value: &[T]) -> Setting {
    let is = |name: &[u8]| value.iter().copied().eq(name.iter().copied().map(T::from));
    if value.is_empty() {
        Setting::Unset
    } else if is(b"system") {
        Setting::Known(Choice::System)
    } else if is(b"snmalloc") {
        Setting::Known(Choice::Snmalloc)
    } else if is(b"mimalloc") {
        Setting::Known(Choice::Mimalloc)
    } else {
        Setting::Unknown
    }
}

#[cfg(unix)]
pub(super) fn read() -> Setting {
    let getenv = |key: &std::ffi::CStr| {
        // SAFETY: The key is NUL-terminated. As with other Unix environment reads,
        // writers must exclude concurrent readers (std::env::set_var).
        unsafe { libc::getenv(key.as_ptr()).cast_const() }
    };
    // SAFETY: On Linux and macOS, concurrent readers do not invalidate the returned C string.
    unsafe { read_with(getenv) }
}

/// # Safety
/// The lookup returns null or a NUL-terminated string that stays live until this function returns.
#[cfg(unix)]
pub(super) unsafe fn read_with(lookup: impl FnOnce(&std::ffi::CStr) -> *const libc::c_char) -> Setting {
    let value = lookup(c"RALLOCATOR_DYNAMIC_PERSONALITY");
    let value = if value.is_null() {
        c""
    } else {
        // SAFETY: The caller guarantees a live NUL-terminated string.
        unsafe { std::ffi::CStr::from_ptr(value) }
    };
    parse(value.to_bytes())
}

// Eight characters for "snmalloc" or "mimalloc", plus the terminator.
#[cfg(windows)]
const CAPACITY: u32 = 9;

#[cfg(windows)]
pub(super) fn read() -> Setting {
    use windows_sys::Win32::System::Environment::GetEnvironmentVariableW;

    read_with(|key, value| {
        // SAFETY: The key is NUL-terminated and the buffer holds CAPACITY UTF-16 units.
        unsafe { GetEnvironmentVariableW(key, value.as_mut_ptr(), CAPACITY) as usize }
    })
}

#[cfg(windows)]
pub(super) fn read_with(lookup: impl FnOnce(*const u16, &mut [u16; CAPACITY as usize]) -> usize) -> Setting {
    let mut value = [0u16; CAPACITY as usize];
    let length = lookup(windows_sys::w!("RALLOCATOR_DYNAMIC_PERSONALITY"), &mut value);
    // Zero denotes an absent or empty value. An oversized result includes the
    // terminator and exceeds our buffer, so never parse a truncated prefix.
    value.get(..length).map_or(Setting::Unknown, parse)
}

pub(super) fn stderr_warning(write: impl FnOnce(Output, Choice)) -> impl FnOnce(Choice) {
    move |fallback| {
        #[cfg(unix)]
        let stderr = libc::STDERR_FILENO;
        #[cfg(windows)]
        let stderr = {
            use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
            // SAFETY: This borrows the process standard error handle without closing it.
            unsafe { GetStdHandle(STD_ERROR_HANDLE) }
        };
        write(stderr, fallback);
    }
}

pub(super) fn write_warning(output: Output, fallback: Choice) {
    let message: &[u8] = match fallback {
        Choice::Snmalloc => b"rallocator: unknown RALLOCATOR_DYNAMIC_PERSONALITY. Use snmalloc, mimalloc, or system. Using snmalloc.\n",
        Choice::Mimalloc => b"rallocator: unknown RALLOCATOR_DYNAMIC_PERSONALITY. Use snmalloc, mimalloc, or system. Using mimalloc.\n",
        Choice::System => b"rallocator: unknown RALLOCATOR_DYNAMIC_PERSONALITY. Use snmalloc, mimalloc, or system. Using system.\n",
    };
    // Best effort: avoid Rust's stderr lock and do not unwind if the destination is closed.
    #[cfg(unix)]
    // SAFETY: write borrows the static message for its exact byte length.
    unsafe {
        libc::write(output, message.as_ptr().cast(), message.len());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::WriteFile;
        let count = u32::try_from(message.len()).unwrap_or(u32::MAX);
        let mut written = 0;
        // SAFETY: WriteFile borrows the message and count for a synchronous write.
        unsafe { WriteFile(output, message.as_ptr(), count, &raw mut written, std::ptr::null_mut()) };
    }
}
