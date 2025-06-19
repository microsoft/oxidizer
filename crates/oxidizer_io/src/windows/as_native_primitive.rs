// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinSock::SOCKET;

use crate::AsNativePrimitive;

/// (Windows only) Extensions to types that represent native I/O primitives. This helps project
/// the abstract primitives into platform-specific types like `SOCKET` and `HANDLE`.
pub trait AsNativePrimitiveExt {
    /// (Windows only) Views the primitive as a `HANDLE`. This is always a valid projection for
    /// Windows primitives.
    ///
    /// # Resource management
    ///
    /// The returned `HANDLE` and any copy made from it is only valid for the lifetime of the
    /// returned reference. Using the handle after the reference lifetime ends does not violate
    /// memory safety but may yield unexpected results (e.g. I/O operations may unexpectedly fail
    /// or be performed on the wrong files).
    ///
    /// # Thread safety
    ///
    /// The returned `HANDLE` is natively single-threaded. It is not valid to use the returned
    /// handle on a different thread.
    ///
    /// Note that the I/O subsystem allows you to obtain a `HANDLE` for the same primitive on
    /// any thread - the important thing is that you call this method on the thread you want
    /// to use the `HANDLE` on.
    fn as_handle(&self) -> &HANDLE;

    /// (Windows only) Views the primitive as a `SOCKET` if this is a valid projection for it.
    ///
    /// # Resource management
    ///
    /// The returned `SOCKET` and any copy made from it is only valid for the lifetime of the
    /// returned reference. Using the socket after the reference lifetime ends does not violate
    /// memory safety but may yield unexpected results (e.g. I/O operations may unexpectedly fail
    /// or be performed on the wrong sockets).
    ///
    /// # Thread safety
    ///
    /// A `SOCKET` is natively defined as a thread-safe type. Nevertheless, the lifetime constraint
    /// in the resource management section implies that the returned socket cannot be copied and
    /// moved to another thread (because then there is no way to guarantee that the reference
    /// outlives the copy of the handle).
    ///
    /// Note that the I/O subsystem allows you to obtain a `SOCKET` for the same primitive on
    /// any thread - the important thing is that you call this method on the thread you want
    /// to use the `SOCKET` on.
    fn try_as_socket(&self) -> Option<&SOCKET>;
}

impl<T> AsNativePrimitiveExt for T
where
    T: AsNativePrimitive,
{
    fn as_handle(&self) -> &HANDLE {
        self.as_pal_primitive().as_real().as_handle()
    }

    fn try_as_socket(&self) -> Option<&SOCKET> {
        self.as_pal_primitive().as_real().try_as_socket()
    }
}