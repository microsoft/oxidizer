// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Byte sequence and memory management primitives of the Oxidizer I/O subsystem, with the most
//! important types being:
//!
//! * [`Sequence`] - a sequence of immutable bytes stored in I/O memory, typically either the result
//!   of an operation that reads bytes of data or some data that the application has prepared for
//!   consumption by an I/O operation (e.g. writing to a file).
//! * [`SequenceBuilder`] - has write access to some amount of I/O memory and can be used to fill
//!   the memory with data to yield [`Sequence`]s; used both to prepare data to be written (e.g.
//!   when writing to a file) and to provide the memory for any data to be read by an I/O
//!   operation (e.g. when reading from file).
//!
//! All byte sequences read or written during I/O operations are stored in memory owned by the
//! I/O subsystem - reading or writing via caller-supplied memory is explicitly not supported.
//!
//! When an app wants to prepare some data to be written by an I/O endpoint, the typical pattern is
//! to ask that I/O endpoint for some memory to place the data into. The I/O endpoint will then
//! prepare a [`SequenceBuilder`] whose memory layout is optimized for the I/O endpoint, and
//! which can be used by the caller to build a [`Sequence`] containing the data to be written.
//!
//! ```ignore
//! // Pseudocode using an imaginary `File` type that is not part of this crate.
//! let mut file = File::create("example.txt", &io_context).await?;
//!
//! // Prepares a byte sequence to be written into the file.
//! // The `File` type will ensure an optimal memory layout is used, suitable for file I/O.
//! let mut sequence_builder = file.memory().reserve(2048);
//! sequence_builder.put(b"Hello, world!");
//!
//! // Consumes all the data in the builder and returns it as a sequence of immutable bytes.
//! let sequence = sequence_builder.consume_all();
//!
//! // Writes the byte sequence to the file.
//! file.write(sequence).await?;
//!
//! // Closes the file, ensuring that all data gets flushed without errors.
//! file.close().await?;
//! ```
//!
//! The data inside a [`Sequence`] is immutable, and instances of [`Sequence`] can be sliced
//! or cloned without additional memory allocation.
//!
//! For compatibility with the popular `bytes` crate, [`Sequence::into_bytes()`][16] can be used to
//! transform a `Sequence` into a [`Bytes`][18] instance, though this is not always zero-copy.
//! [`Sequence`] also implements the [`bytes::buf::Buf`][17] trait for easy consumption of
//! the data within. Likewise, [`SequenceBuilder`] implements the [`bytes::buf::BufMut`][20]
//! trait to make it easy to emit desired byte sequences.
//!
//! [16]: crate::Sequence::into_bytes
//! [17]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html
//! [18]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
//! [20]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html

mod block;
mod memory_guard;

mod memory_pool;
mod provide_memory;
mod sequence;
mod sequence_builder;
mod span;
mod span_builder;
mod thread_safe;

pub(crate) use block::{Block, MAX_BLOCK_SIZE};
pub(crate) use span::Span;
pub(crate) use span_builder::{InspectSpanBuilderData, SpanBuilder};
pub(crate) use thread_safe::ThreadSafe;

pub use memory_guard::MemoryGuard;
pub use memory_pool::DefaultMemoryPool;
pub use provide_memory::ProvideMemory;
pub use sequence::Sequence;
pub use sequence_builder::{
    SequenceBuilder, SequenceBuilderAvailableIterator, SequenceBuilderInspector,
    SequenceBuilderVectoredWrite,
};

#[cfg(any(feature = "fakes", test))]
mod fake_memory_provider;

#[cfg(any(feature = "fakes", test))]
pub use fake_memory_provider::FakeMemoryProvider;

#[cfg(test)]
mod testing;