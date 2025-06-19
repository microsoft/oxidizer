// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Indicates the manner in which a `begin()` call on an I/O operation completed,
/// with an inner result `R` potentially available immediately.
///
/// This distinction is important because even when using asynchronous I/O, operations may still
/// complete synchronously if there is no need to wait for something (e.g. because all the data is
/// already in some operating system buffers).
///
/// The I/O subsystem uses this information to short-circuit the operation completion process and
/// avoid a potentially more costly asynchronous path when a synchronous path is sufficient.
#[derive(Debug)]
pub enum BeginResult<R> {
    /// The operation completed synchronously (with either success or failure) and did not initiate
    /// any asynchronous work.
    ///
    /// For operations that read or write data, the caller must indicate the number of
    /// bytes transferred via `args.bytes_read_synchronously_as_mut()`
    /// or `args.bytes_written_synchronously_as_mut()` when returning this result.
    ///
    /// By returning this, an I/O operation callback given to `*Operation::begin()` promises that
    /// no resources owned by the operation will be used by the operating system after this result
    /// is returned. As part of this the synthetic `'operation` lifetime used in `args` ends now.
    ///
    /// Resources in scope of this requirement include:
    ///
    /// * Any I/O memory buffers associated with the operation.
    /// * Platform-specific operation data (e.g. `args.overlapped()` on Windows).
    /// * Any members of the `begin()` callback's `args` that are marked with the `'operation`
    ///   lifetime.
    CompletedSynchronously(crate::Result<R>),

    /// The operation has started and will be completed asynchronously, with the operating system
    /// notifying us when the operation has completed.
    ///
    /// By returning this, an I/O operation callback given to `*Operation::begin()` indicates that
    /// the operating system has taken a dependency on all resources owned by the operation and that
    /// the resources must not be released until an asynchronous completion notification is received
    /// for the operation.
    ///
    /// Once the operating system signals completion of the asynchronous operation, the resources
    /// associated with this elementary operation must no longer be accessed by user code nor the
    /// operating system.
    ///
    /// Resources in scope of this requirement include:
    ///
    /// * Any I/O memory buffers associated with the operation.
    /// * Platform-specific operation data (e.g. `args.overlapped()` on Windows).
    /// * Any members of the `begin()` callback's `args` that are marked with the `'operation`
    ///   lifetime.
    Asynchronous,
}