// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use bytesbuf::BytesBuf;
use bytesbuf::mem::{HasMemory, Memory};

/// Allows for reading of bytes.
///
/// Only supports asynchronous access, exposing the read byte sequences as `bytesbuf::BytesBuf`.
///
/// # Ownership
///
/// The methods on this trait accept `&mut self` and take an exclusive reference to the source for
/// the duration of the operation. This implies that only one read operation can be concurrently
/// executed on the object.
///
/// # Memory management for efficient I/O
///
/// For optimal efficiency when performing I/O, reads should be performed into memory optimized
/// for the underlying I/O endpoint. This is achieved by reserving memory from the implementation's
/// memory provider before performing the read operation.
///
/// There are three ways to ensure you are using memory suitable for optimally efficient I/O:
///
/// 1. If you call methods that do not accept a `BytesBuf` (such as `read_any()`), the
///    implementation will reserve memory from its memory provider internally. This is the simplest way
///    to perform reads but only a limited API surface is available in this mode.
/// 2. You may call [`Memory::reserve()`][2] on the implementation to reserve memory from its memory
///    provider explicitly. This allows you to control the memory allocation more finely and
///    potentially reuse existing buffers, improving efficiency.
/// 3. You may sometimes want to call `reserve()` at certain times when Rust borrowing rules do
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
pub trait Read: HasMemory + Memory + Debug {
    /// Type used to signal errors by the implementation of this trait.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reads at most `len` bytes into the provided buffer.
    ///
    /// It is not necessary for `into` to be empty - the buffer may already have some
    /// bytes of data in it (e.g. from a previous read).
    ///
    /// The buffer will be extended with additional memory capacity
    /// if it does not have enough remaining capacity to fit `len` additional bytes.
    ///
    /// Returns a tuple of the number of bytes read and the updated buffer.
    ///
    /// The returned [`BytesBuf`] will have 0 or more bytes of data appended to it on success,
    /// with 0 appended bytes indicating that no more bytes can be read from this source. Any
    /// data that was already in the buffer will remain untouched.
    ///
    /// # Example
    ///
    /// ```
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::Null;
    /// use bytesbuf::mem::Memory;
    /// use bytesbuf_io::Read;
    ///
    /// # fn get_source() -> Null { Null::new() }
    /// let mut source = get_source();
    ///
    /// let buf = source.reserve(100);
    ///
    /// let (bytes_read, mut buf) = source.read_at_most_into(10, buf).await.unwrap();
    ///
    /// assert!(bytes_read <= 10);
    ///
    /// let bytes = buf.consume_all();
    /// # }));
    /// ```
    async fn read_at_most_into(&mut self, len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error>;

    /// Reads an unspecified number of bytes into the provided buffer.
    ///
    /// The implementation will decide how many bytes to read based on its internal understanding of
    /// what is optimal for sustained throughput at high efficiency. This may be a fixed size,
    /// or it may be a variable size based on the current state of the source.
    ///
    /// It is not necessary for `into` to be empty - the buffer may already have some
    /// bytes of data in it (e.g. from a previous read).
    ///
    /// The buffer will be extended with additional memory capacity
    /// if it does not have enough remaining capacity to fit `len` additional bytes.
    ///
    /// Returns a tuple of the number of bytes read and the updated buffer.
    ///
    /// The returned [`BytesBuf`] will have 0 or more bytes of data appended to it on success,
    /// with 0 appended bytes indicating that no more bytes can be read from this source. Any
    /// data that was already in the buffer will remain untouched.
    ///
    /// # Example
    ///
    /// ```
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::Null;
    /// use bytesbuf_io::Read;
    ///
    /// # fn get_source() -> Null { Null::new() }
    /// let mut source = get_source();
    ///
    /// let mut buf = source.read_any().await.unwrap();
    ///
    /// if buf.len() == 0 {
    ///     println!("Source ended immediately - nothing we can do.");
    ///     return;
    /// }
    ///
    /// loop {
    ///     // We want at least 1 MB of data, read in whatever sized pieces the I/O endpoint
    ///     // considers optimal.
    ///     if buf.len() >= 1024 * 1024 {
    ///         println!("Got 1 MB of data");
    ///         break;
    ///     }
    ///
    ///     let (bytes_read, new_buf) = source.read_more_into(buf).await.unwrap();
    ///     buf = new_buf;
    ///
    ///     if bytes_read == 0 {
    ///         println!("Source ended - no more data before we reached 1 MB");
    ///         break;
    ///     }
    /// }
    /// # }));
    /// ```
    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error>;

    /// Reads an unspecified number of bytes as a new buffer.
    ///
    /// The implementation will decide how many bytes to read based on its internal understanding of
    /// what is optimal for sustained throughput at high efficiency. This may be a fixed size,
    /// or it may be a variable size based on the current state of the source.
    ///
    /// The returned [`BytesBuf`] will have 0 or more bytes of data appended to it on success,
    /// with 0 appended bytes indicating that no more bytes can be read from this source.
    ///
    /// # Security
    ///
    /// **This method is insecure if the side producing the bytes is not trusted**. An attacker
    /// may trickle data byte-by-byte, consuming a large amount of I/O resources.
    ///
    /// Robust code working with untrusted sources should take precautions such as only processing
    /// read data when either a time or length threshold is reached and reusing buffers that
    /// have remaining capacity, appending additional data to existing buffers using
    /// [`read_more_into()`][crate::Read::read_more_into] instead of reserving new buffers
    /// for each read operation.
    ///
    /// # Example
    ///
    /// ```
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::Null;
    /// use bytesbuf_io::Read;
    ///
    /// # fn get_source() -> Null { Null::new() }
    /// let mut source = get_source();
    ///
    /// let buf = source.read_any().await.unwrap();
    ///
    /// println!("first read produced {} bytes of data", buf.len());
    /// # }));
    /// ```
    async fn read_any(&mut self) -> Result<BytesBuf, Self::Error>;
}
