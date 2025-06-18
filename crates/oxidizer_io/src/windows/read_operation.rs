// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ptr;

use windows::Win32::System::IO::OVERLAPPED;

use crate::ReadOperationArgs;

impl ReadOperationArgs<'_, '_> {
    /// (Windows only) A pointer to an `OVERLAPPED` structure that must be passed to the operating
    /// system when an I/O syscall is issued.
    ///
    /// See [`BeginResult`][1] to understand the resource management implications of this field.
    ///
    /// This is an internal structure used by the I/O driver and must be treated as opaque data,
    /// passed to the operating system without modification.
    ///
    /// # Panics
    ///
    /// Panics if called more than once. Each I/O operation on Windows must result in exactly one
    /// asynchronous system call, using the `OVERLAPPED` structure obtained from here exactly once.
    ///
    /// # Safety
    ///
    /// The returned structure must not be reused for multiple system calls.
    ///
    /// The returned structure must only be used with the same I/O primitive that
    /// the `ControlOperationArgs` is associated with.
    ///
    /// [1]: crate::BeginResult
    pub unsafe fn overlapped(&mut self) -> *mut OVERLAPPED {
        // The Windows PAL contract in ElementaryOperationImpl guarantees that the
        // elementary operation key is a pointer to the OVERLAPPED structure inside it.
        ptr::with_exposed_provenance::<OVERLAPPED>(self.consume_elementary_operation_key().0)
            .cast_mut()
    }
}