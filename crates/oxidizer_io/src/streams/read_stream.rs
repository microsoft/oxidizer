// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::mem::{ProvideMemory, SequenceBuilder};

/// A stream of bytes that can be read from.
///
/// # I/O memory
///
/// The I/O endpoint that implements this trait can also [provide the memory][1] for preparing
/// the data for writing, as specific I/O endpoints require specific memory optimizations for best
/// performance. This is orchestrated via [`ProvideMemory`], which is a supertrait of `ReadStream`.
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
pub trait ReadStream: ProvideMemory + Debug {
    /// Reads at most `len` bytes from the stream into the provided sequence builder.
    ///
    /// It is not necessary for `into` to be empty - the sequence builder may already have some
    /// bytes of data in it (e.g. from a previous read). The sequence builder will be extended
    /// with additional capacity if it does not have enough capacity to fit `len` additional bytes.
    ///
    /// Returns a tuple of the number of bytes read and the updated sequence builder.
    ///
    /// The returned [`SequenceBuilder`] will have 0 or more bytes of data appended to it
    /// on success, with 0 appended bytes indicating end of stream. Any existing data will
    /// remain untouched.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// use bytes::BufMut;
    /// use oxidizer_io::{NullStream, ReadStream};
    /// use oxidizer_io::mem::{ProvideMemory};
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// let sequence_builder = stream.reserve(100);
    ///
    /// let (bytes_read, mut sequence_builder) = stream
    ///     .read_at_most_into(10, sequence_builder).await.unwrap();
    ///
    /// assert!(bytes_read <= 10);
    ///
    /// let sequence = sequence_builder.consume_all();
    /// # }));
    /// ```
    async fn read_at_most_into(
        &mut self,
        len: usize,
        into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)>;

    /// Reads an unspecified number of bytes from the stream into the provided sequence builder.
    ///
    /// The I/O endpoint will decide how many bytes to read based on its internal understanding of
    /// what is optimal. This may be a fixed size, or it may be a variable size based on the
    /// current state of the stream.
    ///
    /// It is not necessary for `into` to be empty - the sequence builder may already have some
    /// bytes of data in it (e.g. from a previous read). The sequence builder will be extended
    /// with additional capacity if it does not have enough capacity to fit `len` additional bytes.
    ///
    /// Returns a tuple of the number of bytes read and the updated sequence builder.
    ///
    /// The returned [`SequenceBuilder`] will have 0 or more bytes of data appended to it
    /// on success, with 0 appended bytes indicating end of stream. Any existing data will
    /// remain untouched.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::NullStream;
    /// use oxidizer_io::ReadStream;
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// let mut sequence_builder = stream.read_any().await.unwrap();
    ///
    /// if sequence_builder.len() == 0 {
    ///     println!("Stream ended immediately - nothing we can do.");
    ///     return;
    /// }
    ///
    /// loop {
    ///     // We want at least 1 MB of data, read in whatever sized pieces the I/O endpoint
    ///     // considers optimal.
    ///     if sequence_builder.len() >= 1024 * 1024 {
    ///         println!("Got 1 MB of data");
    ///         break;
    ///     }
    ///
    ///     let (bytes_read, new_sequence_builder) = stream
    ///         .read_more_into(sequence_builder).await.unwrap();
    ///     sequence_builder = new_sequence_builder;
    ///
    ///     if bytes_read == 0 {
    ///         println!("Stream ended - no more data before we reached 1 MB");
    ///         break;
    ///     }
    /// }
    /// # }));
    /// ```
    async fn read_more_into(
        &mut self,
        into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)>;

    /// Reads an unspecified number of bytes from the stream as a new sequence builder.
    ///
    /// The I/O endpoint will decide how many bytes to read based on its internal understanding of
    /// what is optimal. This may be a fixed size, or it may be a variable size based on the
    /// current state of the stream.
    ///
    /// The returned `SequenceBuilder` will contain 0 or more bytes of read data from the
    /// stream on success, with 0 bytes indicating end of stream.
    ///
    /// # Security
    ///
    /// This method is not safe if the the other side of the stream is not trusted. An attacker
    /// may trickle data byte-by-byte, consuming a large amount of I/O resources.
    ///
    /// Robust code working with untrusted streams should take precautions such as only processing
    /// read data when either a time or length threshold is reached and reusing byte sequences that
    /// have remaining capacity, meanwhile appending to existing memory using
    /// [`read_more_into()`][crate::ReadStream::read_more_into] instead of reserving new memory
    /// for each read operation.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::NullStream;
    /// use oxidizer_io::ReadStream;
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// let sequence_builder = stream.read_any().await.unwrap();
    ///
    /// println!("first read produced {} bytes of data", sequence_builder.len());
    /// # }));
    /// ```
    async fn read_any(&mut self) -> crate::Result<SequenceBuilder>;
}