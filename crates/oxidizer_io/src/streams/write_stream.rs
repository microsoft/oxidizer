// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::mem::{ProvideMemory, Sequence};

/// A stream of bytes that can be written to.
///
/// # I/O memory
///
/// The I/O endpoint that implements this trait can also [provide the memory][1] for preparing
/// the data for writing, as specific I/O endpoints require specific memory optimizations for best
/// performance. This is orchestrated via [`ProvideMemory`], which is a supertrait of `WriteStream`.
///
/// # Ownership
///
/// The methods on this trait accept `&mut self` and take an exclusive reference to the stream for
/// the duration of the operation. This implies that only one concurrent I/O operation can be
/// executed on a stream.
///
/// To implement an I/O endpoint that supports concurrent write operations, you need to model the
/// endpoint in a way that exposes a separate object (e.g. an "operation") for each concurrent
/// stream operation.
///
/// # I/O mmemory manager
///
/// Operations are valid when performed using data stored in any I/O memory associated with the same
/// I/O memory manager.
///
/// For example, if copying from a network socket to a file, you would likely want to read into I/O
/// memory allocated by the network socket and then submit those buffers (optimized for network
/// sockets) to be written to a file (or vice versa, reading into filesystem-optimized buffers).
/// It would not make sense in this scenario to copy from network socket buffers to file buffers
/// as the copy would be more expensive than any gains from using endpoint-optimized memory.
///
/// If you need to operate on a byte sequence that come from a different I/O memory manager,
/// however, you must reserve new memory capacity and copy the data over. Typically, all I/O
/// contexts exposed by the same async task runtime will share the same I/O memory manager, so
/// this would only come up in highly specialized cases.
///
/// # Thread safety
///
/// This trait requires `Send` from both the implementation and any returned futures.
///
/// [1]: crate::mem::ProvideMemory
#[trait_variant::make(Send)]
pub trait WriteStream: ProvideMemory {
    /// Writes the provided byte sequence to the stream.
    ///
    /// The method completes when all bytes have been written. Partial writes are considered
    /// a failure.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::NullStream;
    /// use bytes::BufMut;
    /// use oxidizer_io::WriteStream;
    /// use oxidizer_io::mem::{ProvideMemory};
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// let mut sequence_builder = stream.reserve(100);
    /// sequence_builder.put_slice(b"Hello, world!");
    /// let sequence = sequence_builder.consume_all();
    ///
    /// stream.write(sequence).await.unwrap();
    /// # }));
    /// ```
    async fn write(&mut self, sequence: Sequence) -> crate::Result<()>;
}