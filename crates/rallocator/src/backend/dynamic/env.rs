// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Startup lookup must not allocate through Rust's global allocator. In
//! particular, `std::env::var_os` allocates an owned string for a present value.

use super::Choice;

fn parse<T: Copy + PartialEq + From<u8>>(value: &[T]) -> Option<Choice> {
    let is = |name: &[u8]| value.iter().copied().eq(name.iter().copied().map(T::from));
    if value.is_empty() || is(b"snmalloc") {
        Some(Choice::Snmalloc)
    } else if is(b"mimalloc") {
        Some(Choice::Mimalloc)
    } else if is(b"system") {
        Some(Choice::System)
    } else {
        None
    }
}

#[cfg(unix)]
pub(super) fn read() -> Option<Choice> {
    // SAFETY: The key is a static C string. On Linux and macOS, concurrent
    // readers do not invalidate getenv's result. As with all Unix environment
    // reads, a writer must exclude concurrent readers (std::env::set_var).
    let value = unsafe { libc::getenv(c"RALLOCATOR_DYNAMIC_PERSONALITY".as_ptr()) };
    if value.is_null() {
        return Some(Choice::Snmalloc);
    }
    // SAFETY: getenv returned a live NUL-terminated value, borrowed only for this lookup.
    parse(unsafe { std::ffi::CStr::from_ptr(value) }.to_bytes())
}

#[cfg(windows)]
pub(super) fn read() -> Option<Choice> {
    use windows_sys::Win32::System::Environment::GetEnvironmentVariableW;
    use windows_sys::w;

    // Eight characters for "snmalloc" or "mimalloc", plus the terminator.
    const CAPACITY: u32 = 9;
    let mut value = [0u16; CAPACITY as usize];
    // SAFETY: The name is NUL-terminated and the buffer holds the supplied number of UTF-16 units.
    let length = unsafe { GetEnvironmentVariableW(w!("RALLOCATOR_DYNAMIC_PERSONALITY"), value.as_mut_ptr(), CAPACITY) } as usize;
    // Zero denotes an absent or empty value. An oversized result includes the
    // terminator and exceeds our buffer, so never parse a truncated prefix.
    value.get(..length).and_then(parse)
}

pub(super) fn warn_invalid() {
    const MESSAGE: &[u8] = b"rallocator: unknown RALLOCATOR_DYNAMIC_PERSONALITY. Use snmalloc, mimalloc, or system. Using snmalloc.\n";
    // Best effort: avoid Rust's stderr lock and do not unwind if stderr is closed.
    #[cfg(unix)]
    // SAFETY: write only borrows the static message for its exact byte length.
    unsafe {
        libc::write(libc::STDERR_FILENO, MESSAGE.as_ptr().cast(), MESSAGE.len());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::WriteFile;
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};

        // SAFETY: This borrows the process standard error handle without closing it.
        let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        let count = u32::try_from(MESSAGE.len()).unwrap_or(u32::MAX);
        let mut written = 0;
        // SAFETY: WriteFile borrows the message and count for a synchronous write.
        // A missing or invalid standard error handle causes the write to fail.
        unsafe {
            WriteFile(stderr, MESSAGE.as_ptr(), count, &raw mut written, std::ptr::null_mut());
        }
    }
}
