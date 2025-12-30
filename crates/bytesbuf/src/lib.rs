// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]

//! Create and manipulate byte sequences for efficient I/O.
//!
//! A byte sequence is a logical sequence of zero or more bytes stored in memory,
//! similar to a slice `&[u8]` but with some key differences:
//!
//! * The bytes in a byte sequence are not required to be consecutive in memory.
//! * The bytes in a byte sequence are always immutable.
//!
//! In practical terms, you may think of a byte sequence as a `Vec<Vec<u8>>` whose contents are
//! treated as one logical sequence of bytes. Byte sequences are created via [`BytesBuf`] and
//! consumed via [`BytesView`].
//!
//! # Consuming Byte Sequences
//!
//! A byte sequence is typically consumed by reading its contents. This is done via the
//! [`BytesView`] type, which is a view over a byte sequence. When reading data, the read
//! bytes are removed from the view, shrinking it to only the remaining bytes.
//!
//! There are many helper methods on this type for easily consuming bytes from the view:
//!
//! * [`get_num_le::<T>()`] reads numbers. Big-endian/native-endian variants also exist.
//! * [`get_byte()`] reads a single byte.
//! * [`copy_to_slice()`] copies bytes into a provided slice.
//! * [`copy_to_uninit_slice()`] copies bytes into a provided uninitialized slice.
//! * [`as_read()`] creates a `std::io::Read` adapter for reading bytes via standard I/O methods.
//!
//! ```
//! # let memory = bytesbuf::GlobalPool::new();
//! # let message = BytesView::copied_from_slice(b"1234123412341234", &memory);
//! use bytesbuf::BytesView;
//!
//! fn consume_message(mut message: BytesView) {
//!     // We read the message and calculate the sum of all the words in it.
//!     let mut sum: u64 = 0;
//!
//!     while !message.is_empty() {
//!         let word = message.get_num_le::<u64>();
//!         sum = sum.saturating_add(word);
//!     }
//!
//!     println!("Message received. The sum of all words in the message is {sum}.");
//! }
//! # consume_message(message);
//! ```
//!
//! If the helper methods are not sufficient, you can access the byte sequence via byte slices using the
//! following fundamental methods that underpin the convenience methods:
//!
//! * [`first_slice()`], which returns the first slice of bytes that makes up the byte sequence. The
//!   length of this slice is determined by the inner structure of the byte sequence and it may not
//!   contain all the bytes.
//! * [`advance()`][ViewAdvance], which marks bytes from the beginning of [`first_slice()`] as read, shrinking the
//!   view of the byte sequence by the corresponding amount and moving remaining data up to the front.
//!   When you advance past the slice returned by [`first_slice()`], the next call to [`first_slice()`]
//!   will return a new slice of bytes starting from the new front position of the view.
//!
//! ```
//! # let memory = bytesbuf::GlobalPool::new();
//! # let mut bytes = BytesView::copied_from_slice(b"1234123412341234", &memory);
//! use bytesbuf::BytesView;
//!
//! let len = bytes.len();
//! let mut slice_lengths = Vec::new();
//!
//! while !bytes.is_empty() {
//!     let slice = bytes.first_slice();
//!     slice_lengths.push(slice.len());
//!
//!     // We have completed processing this slice. All we wanted was to know its length.
//!     // We can now mark this slice as consumed, revealing the next slice for inspection.
//!     bytes.advance(slice.len());
//! }
//!
//! println!("Inspected a view over {len} bytes with slice lengths: {slice_lengths:?}");
//! ```
//!
//! To reuse a byte sequence, clone it before consuming the contents. This is a cheap
//! zero-copy operation.
//!
//! ```
//! # let memory = bytesbuf::GlobalPool::new();
//! # let mut bytes = BytesView::copied_from_slice(b"1234123412341234", &memory);
//! use bytesbuf::BytesView;
//!
//! assert_eq!(bytes.len(), 16);
//!
//! let mut bytes_clone = bytes.clone();
//! assert_eq!(bytes_clone.len(), 16);
//!
//! // Consume 8 bytes from the front.
//! _ = bytes_clone.get_num_le::<u64>();
//! assert_eq!(bytes_clone.len(), 8);
//!
//! // Operations on the clone have no effect on the original view.
//! assert_eq!(bytes.len(), 16);
//! ```
//!
//! # Producing Byte Sequences
//!
//! For creating a byte sequence, you first need some memory capacity to put the bytes into. This
//! means you need a memory provider, which is a type that implements the [`Memory`] trait.
//!
//! Obtaining a memory provider is generally straightforward. Simply use the first matching option
//! from the following list:
//!
//! 1. If you are creating byte sequences for the purpose of submitting them to a specific
//!    object of a known type (e.g. writing them to a `TcpConnection`), the target type will
//!    typically implement the [`HasMemory`] trait, which gives you a suitable memory
//!    provider instance via [`HasMemory::memory()`]. Use this as the memory provider - it will
//!    give you memory with the configuration that is optimal for delivering bytes to that
//!    specific consumer.
//! 1. If you are creating byte sequences as part of usage-neutral data processing, obtain an
//!    instance of a shared [`GlobalPool`]. In a typical web application, the global memory pool
//!    is a service exposed by the application framework. In a different context (e.g. example
//!    or test code with no framework), you can create your own instance via `GlobalPool::new()`.
//!
//! Once you have a memory provider, you can reserve memory from it by calling
//! [`Memory::reserve()`] on it. This returns a [`BytesBuf`] with the requested
//! memory capacity.
//!
//! ```
//! # struct Connection {}
//! # impl Connection { fn memory(&self) -> impl Memory { bytesbuf::GlobalPool::new() } }
//! # let connection = Connection {};
//! use bytesbuf::Memory;
//!
//! let memory = connection.memory();
//!
//! let mut buf = memory.reserve(100);
//! ```
//!
//! Now that you have the memory capacity in a [`BytesBuf`], you can fill the memory
//! capacity with bytes of data. Creating byte sequences in a [`BytesBuf`] is an
//! append-only process - you can only add data to the end of the buffered sequence.
//!
//! There are many helper methods on [`BytesBuf`] for easily appending bytes to the buffer:
//!
//! * [`put_num_le::<T>()`], which appends numbers. Big-endian/native-endian variants also exist.
//! * [`put_slice()`], which appends a slice of bytes.
//! * [`put_byte()`], which appends a single byte.
//! * [`put_byte_repeated()`], which appends multiple repetitions of a byte.
//! * [`put_bytes()`], which appends an existing [`BytesView`].
//!
//! ```
//! # struct Connection {}
//! # impl Connection { fn memory(&self) -> impl Memory { bytesbuf::GlobalPool::new() } }
//! # let connection = Connection {};
//! use bytesbuf::Memory;
//!
//! let memory = connection.memory();
//!
//! let mut buf = memory.reserve(100);
//!
//! buf.put_num_be(1234_u64);
//! buf.put_num_be(5678_u64);
//! buf.put_slice(*b"Hello, world!");
//! ```
//!
//! If the helper methods are not sufficient, you can write contents directly into mutable byte slices
//! using the fundamental methods that underpin the convenience methods:
//!
//! * [`first_unfilled_slice()`], which returns a mutable slice of bytes from the beginning of the
//!   buffer's remaining capacity. The length of this slice is determined by the inner memory layout
//!   of the buffer and it may not contain all the capacity that has been reserved.
//! * [`advance()`][BufAdvance], which declares that a number of bytes at the beginning of [`first_unfilled_slice()`]
//!   have been initialized with data and are no longer unused. This will mark these bytes as valid for
//!   consumption and advance [`first_unfilled_slice()`] to the next slice of unused memory capacity
//!   if the current slice has been completely filled.
//!
//! See `examples/bb_slice_by_slice_write.rs` for an example of how to use these methods.
//!
//! If you do not know exactly how much memory you need in advance, you can extend the sequence
//! builder capacity on demand by calling [`BytesBuf::reserve()`]. You can use [`remaining_capacity()`]
//! to identify how much unused memory capacity is available.
//!
//! ```
//! # struct Connection {}
//! # impl Connection { fn memory(&self) -> impl Memory { bytesbuf::GlobalPool::new() } }
//! # let connection = Connection {};
//! use bytesbuf::Memory;
//!
//! let memory = connection.memory();
//!
//! let mut buf = memory.reserve(100);
//!
//! // .. write some data into the buffer ..
//!
//! // We discover that we need 80 additional bytes of memory! No problem.
//! buf.reserve(80, &memory);
//!
//! // Remember that a memory provider can always provide more memory than requested.
//! assert!(buf.capacity() >= 100 + 80);
//! assert!(buf.remaining_capacity() >= 80);
//! ```
//!
//! When you have filled the memory capacity with the contents of the byte sequence, you can consume
//! the data in the buffer as a [`BytesView`] over immutable bytes.
//!
//! ```
//! # struct Connection {}
//! # impl Connection { fn memory(&self) -> impl Memory { bytesbuf::GlobalPool::new() } }
//! # let connection = Connection {};
//! use bytesbuf::Memory;
//!
//! let memory = connection.memory();
//!
//! let mut buf = memory.reserve(100);
//!
//! buf.put_num_be(1234_u64);
//! buf.put_num_be(5678_u64);
//! buf.put_slice(*b"Hello, world!");
//!
//! let message = buf.consume_all();
//! ```
//!
//! This can be done piece by piece, and you can continue writing to the buffer
//! after consuming some already written bytes.
//!
//! ```
//! # struct Connection {}
//! # impl Connection { fn memory(&self) -> impl Memory { bytesbuf::GlobalPool::new() } }
//! # let connection = Connection {};
//! use bytesbuf::Memory;
//!
//! let memory = connection.memory();
//!
//! let mut buf = memory.reserve(100);
//!
//! buf.put_num_be(1234_u64);
//! buf.put_num_be(5678_u64);
//!
//! let first_8_bytes = buf.consume(8);
//! let second_8_bytes = buf.consume(8);
//!
//! buf.put_slice(*b"Hello, world!");
//!
//! let final_contents = buf.consume_all();
//! ```
//!
//! If you already have a [`BytesView`] that you want to write into a [`BytesBuf`], call
//! [`BytesBuf::put_bytes()`]. This is a highly efficient zero-copy operation
//! that reuses the memory capacity of the view you are appending.
//!
//! ```
//! # struct Connection {}
//! # impl Connection { fn memory(&self) -> impl Memory { bytesbuf::GlobalPool::new() } }
//! # let connection = Connection {};
//! use bytesbuf::Memory;
//!
//! let memory = connection.memory();
//!
//! let mut header_builder = memory.reserve(16);
//! header_builder.put_num_be(1234_u64);
//! let header = header_builder.consume_all();
//!
//! let mut buf = memory.reserve(128);
//! buf.put_bytes(header);
//! buf.put_slice(*b"Hello, world!");
//! ```
//!
//! Note that there is no requirement that the memory capacity of the buffer and the
//! memory capacity of the view being appended come from the same memory provider. It is valid
//! to mix and match memory from different providers, though this may disable some optimizations.
//!
//! # Implementing APIs that Consume Byte Sequences
//!
//! If you are implementing a type that accepts byte sequences, you should implement the
//! [`HasMemory`] trait to make it possible for the caller to use optimally
//! configured memory when creating the byte sequences for input to your type.
//!
//! Even if the implementation of your type today is not capable of taking advantage of
//! optimizations that depend on the memory configuration, it may be capable of doing so
//! in the future or may, today or in the future, pass the data to another type that
//! implements [`HasMemory`], which can take advantage of memory optimizations.
//! Therefore, it is best to implement this trait on all types that accept byte sequences.
//!
//! The recommended implementation strategy for [`HasMemory`] is as follows:
//!
//! * If your type always passes the data to another type that implements [`HasMemory`],
//!   simply forward the memory provider from the other type.
//! * If your type can take advantage of optimizations enabled by specific memory configurations,
//!   (e.g. because it uses operating system APIs that unlock better performance when the memory
//!   is appropriately configured), return a memory provider that performs the necessary
//!   configuration.
//! * If your type neither passes the data to another type that implements [`HasMemory`]
//!   nor can take advantage of optimizations enabled by specific memory configurations, obtain
//!   an instance of [`GlobalPool`] as a dependency and return it as the memory provider.
//!
//! Example of forwarding the memory provider (see `examples/bb_has_memory_forwarding.rs`
//! for full code):
//!
//! ```
//! use bytesbuf::{HasMemory, MemoryShared, BytesView};
//!
//! /// Counts the number of 0x00 bytes in a byte sequence before
//! /// writing that byte sequence to a network connection.
//! ///
//! /// # Implementation strategy for `HasMemory`
//! ///
//! /// This type merely inspects a byte sequence before passing it on. This means that it does not
//! /// have a preference of its own for how that memory should be configured.
//! ///
//! /// However, the thing it passes the sequence to (the `Connection` type) may have a preference,
//! /// so we forward the memory provider of the `Connection` type as our own memory provider, so the
//! /// caller can use memory optimal for submission to the `Connection` instance.
//! #[derive(Debug)]
//! struct ConnectionZeroCounter {
//!     connection: Connection,
//! }
//!
//! impl ConnectionZeroCounter {
//!     pub fn new(connection: Connection) -> Self {
//!         Self {
//!             connection,
//!         }
//!     }
//!
//!     pub fn write(&mut self, message: BytesView) {
//!         // TODO: Count zeros.
//!
//!         self.connection.write(message);
//!     }
//! }
//!
//! impl HasMemory for ConnectionZeroCounter {
//!     fn memory(&self) -> impl MemoryShared {
//!         // We forward the memory provider of the connection, so that the caller can use
//!         // memory optimal for submission to the connection.
//!         self.connection.memory()
//!     }
//! }
//! # #[derive(Debug)] struct Connection;
//! # impl Connection { fn write(&mut self, mut _message: BytesView) {} }
//! # impl HasMemory for Connection { fn memory(&self) -> impl MemoryShared { bytesbuf::TransparentTestMemory::new() } }
//! ```
//!
//! Example of returning a memory provider that performs configuration for optimal memory (see
//! `examples/bb_has_memory_optimizing.rs` for full code):
//!
//! ```
//! use bytesbuf::{CallbackMemory, HasMemory, MemoryShared, BytesView};
//!
//! /// # Implementation strategy for `HasMemory`
//! ///
//! /// This type can benefit from optimal performance if specifically configured memory is used and
//! /// the memory is reserved from the I/O memory pool. It uses the I/O context to reserve memory,
//! /// providing a usage-specific configuration when reserving memory capacity.
//! ///
//! /// A callback memory provider is used to attach the configuration to each memory reservation.
//! #[derive(Debug)]
//! struct UdpConnection {
//!     io_context: IoContext,
//! }
//!
//! impl UdpConnection {
//!     pub fn new(io_context: IoContext) -> Self {
//!         Self { io_context }
//!     }
//! }
//!
//! /// Represents the optimal memory configuration for a UDP connection when reserving I/O memory.
//! const UDP_CONNECTION_OPTIMAL_MEMORY_CONFIGURATION: MemoryConfiguration = MemoryConfiguration {
//!     requires_page_alignment: false,
//!     zero_memory_on_release: false,
//!     requires_registered_memory: true,
//! };
//!
//! impl HasMemory for UdpConnection {
//!     fn memory(&self) -> impl MemoryShared {
//!         CallbackMemory::new({
//!             // Cloning is cheap, as it is a service that shares resources between clones.
//!             let io_context = self.io_context.clone();
//!
//!             move |min_len| {
//!                 io_context.reserve_io_memory(min_len, UDP_CONNECTION_OPTIMAL_MEMORY_CONFIGURATION)
//!             }
//!         })
//!     }
//! }
//!
//! # use bytesbuf::BytesBuf;
//! # #[derive(Clone, Debug)]
//! # struct IoContext;
//! # impl IoContext {
//! #     pub fn reserve_io_memory(
//! #         &self,
//! #         min_len: usize,
//! #         _memory_configuration: MemoryConfiguration,
//! #     ) -> BytesBuf {
//! #         todo!()
//! #     }
//! # }
//! # struct MemoryConfiguration { requires_page_alignment: bool, zero_memory_on_release: bool, requires_registered_memory: bool }
//! ```
//!
//! Example of returning a global memory pool when the type is agnostic toward memory configuration
//! (see `examples/bb_has_memory_global.rs` for full code):
//!
//! ```
//! use bytesbuf::{GlobalPool, HasMemory, MemoryShared};
//!
//! /// Calculates a checksum for a given byte sequence.
//! ///
//! /// # Implementation strategy for `HasMemory`
//! ///
//! /// This type does not benefit from any specific memory configuration - it consumes bytes no
//! /// matter what sort of memory they are in. It also does not pass the bytes to some other type.
//! ///
//! /// Therefore, we simply use `GlobalPool` as the memory provider we publish, as this is
//! /// the default choice when there is no specific provider to prefer.
//! #[derive(Debug)]
//! struct ChecksumCalculator {
//!     // The application logic must provide this - it is our dependency.
//!     memory: GlobalPool,
//! }
//!
//! impl ChecksumCalculator {
//!     pub fn new(memory: GlobalPool) -> Self {
//!         Self { memory }
//!     }
//! }
//!
//! impl HasMemory for ChecksumCalculator {
//!     fn memory(&self) -> impl MemoryShared {
//!         // Cloning a memory provider is intended to be a cheap operation, reusing resources.
//!         self.memory.clone()
//!     }
//! }
//! ```
//!
//! It is generally expected that all APIs work with byte sequences using memory from any provider.
//! It is true that in some cases this may be impossible (e.g. because you are interacting directly
//! with a device driver that requires the data to be in a specific physical memory module) but
//! these cases will be rare and must be explicitly documented.
//!
//! If your type can take advantage of optimizations enabled by specific memory configurations,
//! it needs to determine whether a byte sequence actually uses the desired memory configuration.
//! This can be done by inspecting the provided byte sequence and the memory metadata it exposes.
//! If the metadata indicates a suitable configuration, the optimal implementation can be used.
//! Otherwise, the implementation can fall back to a generic implementation that works with any
//! byte sequence.
//!
//! Example of identifying whether a byte sequence uses the optimal memory configuration (see
//! `examples/bb_optimal_path.rs` for full code):
//!
//! ```
//! # struct Foo;
//! use bytesbuf::BytesView;
//!
//! # impl Foo {
//! pub fn write(&mut self, message: BytesView) {
//!     // We now need to identify whether the message actually uses memory that allows us to
//!     // use the optimal I/O path. There is no requirement that the data passed to us contains
//!     // only memory with our preferred configuration.
//!
//!     let use_optimal_path = message.iter_slice_metas().all(|meta| {
//!         // If there is no metadata, the memory is not I/O memory.
//!         meta.is_some_and(|meta| {
//!             // If the type of metadata does not match the metadata
//!             // exposed by the I/O memory provider, the memory is not I/O memory.
//!             let Some(io_memory_configuration) = meta.downcast_ref::<MemoryConfiguration>()
//!             else {
//!                 return false;
//!             };
//!
//!             // If the memory is I/O memory but is not not pre-registered
//!             // with the operating system, we cannot use the optimal path.
//!             io_memory_configuration.requires_registered_memory
//!         })
//!     });
//!
//!     if use_optimal_path {
//!         self.write_optimal(message);
//!     } else {
//!         self.write_fallback(message);
//!     }
//! }
//! # fn write_optimal(&mut self, _message: BytesView) { }
//! # fn write_fallback(&mut self, _message: BytesView) { }
//! # }
//! # struct MemoryConfiguration { requires_registered_memory: bool }
//! ```
//!
//! Note that there is no requirement that a byte sequence consists of homogeneous memory. Different
//! parts of the byte sequence may come from different memory providers, so all chunks must be
//! checked for compatibility.
//!
//! # Compatibility with the `bytes` Crate
//!
//! The popular [`Bytes`] type from the `bytes` crate is often used in the Rust ecosystem to
//! represent simple byte buffers of consecutive bytes. For compatibility with this commonly used
//! type, this crate offers conversion methods to translate between [`BytesView`] and [`Bytes`]:
//!
//! * [`BytesView::to_bytes()`] converts a [`BytesView`] into a [`Bytes`] instance. This
//!   is not always zero-copy because a byte sequence is not guaranteed to be consecutive in memory.
//!   You are discouraged from using this method in any performance-relevant logic path.
//! * `BytesView::from(Bytes)` or `let s: BytesView = bytes.into()` converts a [`Bytes`] instance
//!   into a [`BytesView`]. This is an efficient zero-copy operation that reuses the memory of the
//!   `Bytes` instance.
//!
//! # Static Data
//!
//! You may have static data in your logic, such as the names/prefixes of request/response headers:
//!
//! ```
//! const HEADER_PREFIX: &[u8] = b"Unix-Milliseconds: ";
//! ```
//!
//! Optimal processing of static data requires satisfying multiple requirements:
//!
//! * We want zero-copy processing when consuming this data.
//! * We want to use memory that is optimally configured for the context in which the data is
//!   consumed (e.g. network connection, file, etc).
//!
//! The standard pattern here is to use [`OnceLock`] to lazily initialize a [`BytesView`] from
//! the static data on first use, using memory from a memory provider that is optimal for the
//! intended usage.
//!
//! ```
//! use std::sync::OnceLock;
//!
//! use bytesbuf::BytesView;
//!
//! const HEADER_PREFIX: &[u8] = b"Unix-Milliseconds: ";
//!
//! // We transform the static data into a BytesView on first use, via OnceLock.
//! //
//! // You are expected to reuse this variable as long as the context does not change.
//! // For example, it is typically fine to share this across multiple network connections
//! // because they all likely use the same memory configuration. However, writing to files
//! // may require a different memory configuration for optimality, so you would need a different
//! // `BytesView` for that. Such details will typically be documented in the API documentation
//! // of the type that consumes the `BytesView` (e.g. a network connection or a file writer).
//! let header_prefix = OnceLock::<BytesView>::new();
//!
//! for _ in 0..10 {
//!     let mut connection = Connection::accept();
//!
//!     // The static data is transformed into a BytesView on first use, using memory optimally configured
//!     // for network connections. The underlying principle is that memory optimally configured for one network
//!     // connection is likely also optimally configured for another network connection, enabling efficient reuse.
//!     let header_prefix = header_prefix
//!         .get_or_init(|| BytesView::copied_from_slice(HEADER_PREFIX, &connection.memory()));
//!
//!     // Now we can use the `header_prefix` BytesView in the connection logic.
//!     // Cloning a BytesView is a cheap zero-copy operation.
//!     connection.write(header_prefix.clone());
//! }
//! # struct Connection;
//! # impl Connection {
//! #     fn accept() -> Self { Connection }
//! #     fn memory(&self) -> impl bytesbuf::Memory { bytesbuf::GlobalPool::new() }
//! #     fn write(&self, _sequence: BytesView) {}
//! # }
//! ```
//!
//! Different usages (e.g. file vs network) may require differently configured memory for optimal
//! performance, so you may need a different `BytesView` if the same static data is to be used
//! in different contexts.
//!
//! # Testing
//!
//! For testing purposes (behind `test-util` Cargo feature), this crate exposes some special-purpose
//! memory providers that are not optimized for real-world usage but may be useful to test corner
//! cases of byte sequence processing in your code:
//!
//! * `TransparentTestMemory` - a memory provider that does not add any value, just uses memory
//!   from the Rust global allocator.
//! * `FixedBlockTestMemory` - a variation of the transparent memory provider that limits
//!   each consecutive memory block to a fixed size. This is useful for testing scenarios where
//!   you want to ensure that your code works well even if a byte sequence consists of
//!   non-consecutive memory. You can go down to as low as 1 byte per block!
//!
//! [`get_num_le::<T>()`]: crate::BytesView::get_num_le
//! [`get_byte()`]: crate::BytesView::get_byte
//! [`copy_to_slice()`]: crate::BytesView::copy_to_slice
//! [`copy_to_uninit_slice()`]: crate::BytesView::copy_to_uninit_slice
//! [`as_read()`]: crate::BytesView::as_read
//! [`first_slice()`]: crate::BytesView::first_slice
//! [ViewAdvance]: crate::BytesView::advance
//! [`put_num_le::<T>()`]: crate::BytesBuf::put_num_le
//! [`put_slice()`]: crate::BytesBuf::put_slice
//! [`put_byte()`]: crate::BytesBuf::put_byte
//! [`put_byte_repeated()`]: crate::BytesBuf::put_byte_repeated
//! [`put_bytes()`]: crate::BytesBuf::put_bytes
//! [`first_unfilled_slice()`]: crate::BytesBuf::first_unfilled_slice
//! [BufAdvance]: crate::BytesBuf::advance
//! [`BytesView::to_bytes()`]: crate::BytesView::to_bytes
//! [`Memory`]: crate::Memory
//! [`HasMemory`]: crate::HasMemory
//! [`HasMemory::memory()`]: crate::HasMemory::memory
//! [`GlobalPool`]: crate::GlobalPool
//! [`Bytes`]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
//! [`remaining_capacity()`]: crate::BytesBuf::remaining_capacity
//! [`OnceLock`]: std::sync::OnceLock

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/bytesbuf/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/bytesbuf/favicon.ico")]

mod block;
mod block_ref;
mod buf;
mod buf_put;
mod bytes_compat;
mod callback_memory;
mod constants;
#[cfg(any(test, feature = "test-util"))]
mod fixed_block;
mod global;
mod has_memory;
mod memory;
mod memory_guard;
mod memory_shared;
mod opaque_memory;
mod read_adapter;
mod slice;
mod span;
mod span_builder;
#[cfg(any(test, feature = "test-util"))]
mod transparent;
mod vec;
mod view;
mod view_get;
mod write_adapter;

pub use block::{Block, BlockSize};
pub use block_ref::{BlockRef, BlockRefDynamic, BlockRefDynamicWithMeta, BlockRefVTable};
pub use buf::{BytesBuf, BytesBufAvailableIterator, BytesBufVectoredWrite};
pub use callback_memory::CallbackMemory;
pub use constants::MAX_INLINE_SPANS;
#[cfg(any(test, feature = "test-util"))]
pub use fixed_block::FixedBlockTestMemory;
pub use global::GlobalPool;
pub use has_memory::HasMemory;
pub use memory::Memory;
pub use memory_guard::MemoryGuard;
pub use memory_shared::MemoryShared;
pub use opaque_memory::OpaqueMemory;
pub(crate) use span::Span;
pub(crate) use span_builder::SpanBuilder;
#[cfg(any(test, feature = "test-util"))]
pub use transparent::TransparentTestMemory;
pub use view::{BytesView, BytesViewSliceMetasIterator};
pub(crate) use write_adapter::BytesBufWrite;

#[cfg(test)]
mod testing;

#[cfg(any(test, feature = "test-util"))]
pub(crate) mod std_alloc_block;
