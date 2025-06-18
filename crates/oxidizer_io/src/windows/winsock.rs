// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! (Windows only) Helper functionality for working with the Windows Sockets API.

use std::sync::LazyLock;

use windows::Win32::Networking::WinSock::{WSA_IO_PENDING, WSADATA, WSAGetLastError, WSAStartup};

use crate::BeginResult;

/// Ensures that global state used by Windows Sockets is initialized. Must be called before
/// invoking any Windows Sockets API.
///
/// May be called any number of times. The first call will initialize the data, subsequent calls
/// will be no-ops. The global state is never uninitialized.
///
/// # Panics
///
/// The function will panic if anything goes wrong with initializing Windows Sockets functionality.
#[cfg_attr(test, mutants::skip)] // Impractical to test due to global effects.
pub fn ensure_initialized() {
    *WINSOCK_STARTUP;
}

static WINSOCK_STARTUP: LazyLock<()> = LazyLock::new(|| {
    let mut data = WSADATA::default();
    // Initialize global state for Windows Sockets 2.2, which is the only version in use.
    // We panic if this fails - without Windows Sockets, the process cannot continue.
    //
    // SAFETY: We are passing a valid pointer, which is fine. While Winsock does require us to
    // clean up afterwards, we intentionally do not do this - we expect to keep Winsock active
    // for the entire life of the process.
    status_code_to_result(unsafe { WSAStartup(0x202, &raw mut data) }).expect(
        "a process that requires Winsock cannot continue operation if Winsock initialization fails",
    );
});

/// Converts a Winsock `i32` status code into a `Result`.
// No mutation - messing with FFI status codes can result in unholy mess up to and including UB.
#[cfg_attr(test, mutants::skip)]
pub fn status_code_to_result(status_code: i32) -> crate::Result<()> {
    // For Winsock, 0 is success, anything else is failure.
    // The status code is not used to carry meaningful information.
    if status_code == 0 {
        Ok(())
    } else {
        // SAFETY: Nothing unsafe here, just an FFI call.
        let winsock_error = unsafe { WSAGetLastError() };

        Err(crate::Error::Winsock(winsock_error))
    }
}

/// Converts a Winsock `i32` status code into a [`BeginResult`].
// No mutation - messing with FFI status codes can result in unholy mess up to and including UB.
#[cfg_attr(test, mutants::skip)]
#[must_use]
pub fn status_code_to_begin_result(status_code: i32) -> BeginResult<()> {
    match status_code_to_result(status_code) {
        // This just means "asynchronous operation started", not a real error.
        Err(crate::Error::Winsock(e)) if e == WSA_IO_PENDING => BeginResult::Asynchronous,
        // Anything else means it completed synchronously.
        x => BeginResult::CompletedSynchronously(x),
    }
}

#[cfg(not(miri))] // Miri does not support Windows Sockets.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_code_to_result_smoke_test() {
        assert!(matches!(status_code_to_result(0), Ok(())));
        // We have no expectation for the actual error code because it comes from real Winsock.
        assert!(matches!(
            status_code_to_result(1),
            Err(crate::Error::Winsock(_))
        ));
    }
}