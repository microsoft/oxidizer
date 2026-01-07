// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use bytesbuf::BytesView;
use bytesbuf::mem::{HasMemory, Memory};

/// Allowing for writing of bytes.
///
/// Only supports asynchronous access. Accepts the byte sequences to be written as `bytesbuf::BytesView` instances.
///
/// # Ownership
///
/// The methods on this trait accept `&mut self` and take an exclusive reference to the object for
/// the duration of the operation. This implies that only one write operation can be concurrently
/// executed on the object.
///
/// # Memory management for efficient I/O
///
/// For optimal efficiency when performing I/O, writes should be performed from memory optimized
/// for the underlying I/O endpoint. This is achieved by reserving memory from the implementation's
/// memory provider and generating your bytes into the returned memory buffer before performing the
/// write operation.
///
/// To be clear, the expectation is that whatever data you want to write is placed into the implementation's
/// provided memory buffers right from the start. If your data starts "somewhere else"
/// and must be copied, optimal I/O efficiency cannot be achieved as copying by definition
/// is a form of inefficiency. The data must be born in the memory buffers used for the write operation.
///
/// There are two ways to ensure you are using memory suitable for optimally efficient I/O:
///
/// 1. You may call [`Memory::reserve()`][2] on the implementing type to reserve memory from its memory provider.
/// 2. You may sometimes want to call `reserve()` at certain times when Rust borrowing rules do
///    not allow you to call it directly on the implementation because it has already been borrowed.
///    In this case, you can obtain an independent reference to the memory provider first via
///    [`HasMemory::memory()`][1], which allows you to bypass the need to borrow the implementing object itself.
///
/// Some implementations do not perform real I/O and only move data around in memory. Such
/// implementations typically do not have any special memory requirements and will operate
/// with the same efficiency regardless of which buffers the data is in. Any relaxed behaviors
/// like this will typically be described in the implementation's API documentation.
///
/// # Thread safety
///
/// This trait requires `Send` from both the implementation and any returned futures.
///
/// [1]: bytesbuf::mem::HasMemory::memory
/// [2]: bytesbuf::mem::Memory::reserve
#[trait_variant::make(Send)]
pub trait Write: HasMemory + Memory + Debug {
    /// Type used to signal errors by the implementation of this trait.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Writes the provided byte sequence.
    ///
    /// The method completes when all bytes have been written.
    /// Partial writes are considered a failure.
    ///
    /// # Example
    ///
    /// ```
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::Null;
    /// use bytesbuf::mem::Memory;
    /// use bytesbuf_io::Write;
    ///
    /// # fn get_sink() -> Null { Null::new() }
    /// let mut sink = get_sink();
    ///
    /// let mut buf = sink.reserve(100);
    /// buf.put_slice(*b"Hello, world!");
    /// let bytes = buf.consume_all();
    ///
    /// sink.write(bytes).await.unwrap();
    /// # }));
    /// ```
    async fn write(&mut self, data: BytesView) -> Result<(), Self::Error>;
}
