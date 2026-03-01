// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/file/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/file/favicon.ico")]

//! Zero-copy asynchronous filesystem API.
//!
//! This crate provides a filesystem API that differs from [`std::fs`] in three key ways:
//!
//! 1. **Capability-based access control.** All filesystem operations are scoped to a
//!    [`Directory`] capability obtained via [`Root::bind`]. Paths are always relative
//!    to a directory, and path traversals that would escape the directory (such as
//!    leading `/` or `..` above the root) are rejected. This makes it possible to
//!    grant a subsystem access to a specific directory tree without risking access
//!    to the rest of the filesystem.
//!
//! 2. **Fully asynchronous.** Every I/O operation is `async`. The implementation uses
//!    a pool of dedicated background threads to perform blocking filesystem calls,
//!    keeping the async executor free.
//!
//! 3. **Managed buffers via [`bytesbuf`].** Reads produce
//!    [`BytesView`](bytesbuf::BytesView) values backed by pooled memory; writes
//!    accept them. This enables zero-copy data pipelines: data read from a file can
//!    be written to a socket (or another file) without intermediate copies, as long
//!    as both endpoints share a compatible memory provider.
//!
//! # Quick start
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use file::Root;
//!
//! // Bind to a directory — the only place an absolute path is accepted.
//! let dir = Root::bind("/var/data").await?;
//!
//! // Read and write whole files through the Directory capability.
//! dir.write_slice("greeting.txt", b"Hello!").await?;
//! let text = dir.read_to_string("greeting.txt").await?;
//!
//! // Narrow the capability to a subdirectory.
//! let sub = dir.open_dir("subdir").await?;
//! let data = sub.read("nested_file.txt").await?;
//! # Ok(())
//! # }
//! ```
//!
//! # File types
//!
//! The crate provides **six file types** organized into two families. Within each
//! family, three types enforce read, write, or read-write access at the type level.
//!
//! ## Seekable files — streaming I/O with a cursor
//!
//! Seekable files maintain an internal cursor that advances with each read or
//! write. They implement [`bytesbuf_io::Read`] and/or [`bytesbuf_io::Write`] for
//! streaming I/O and support [`seek`](File::seek),
//! [`stream_position`](File::stream_position), and [`rewind`](File::rewind).
//!
//! Because the cursor is shared mutable state, all I/O methods take **`&mut self`**,
//! ensuring only one operation is in flight at a time. This makes seekable files
//! ideal for sequential processing: reading a log from top to bottom, writing a
//! report line by line, or appending to a file.
//!
//! | Type | Access | Obtained via |
//! |------|--------|-------------|
//! | [`ReadOnlyFile`]  | Read + seek  | [`ReadOnlyFile::open`] |
//! | [`WriteOnlyFile`] | Write + seek | [`WriteOnlyFile::create`], [`WriteOnlyFile::create_new`] |
//! | [`File`]          | Read + write + seek | [`File::open`], [`File::create`], [`OpenOptions`] |
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use file::{ReadOnlyFile, Root};
//!
//! let dir = Root::bind("/var/data").await?;
//! let mut file = ReadOnlyFile::open(&dir, "log.txt").await?;
//!
//! // Stream through the file in 8 KB chunks.
//! loop {
//!     let chunk = file.read_max(8192).await?;
//!     if chunk.is_empty() {
//!         break; // EOF
//!     }
//!     // process chunk...
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Positional files — offset-based I/O without a cursor
//!
//! Positional files have **no cursor**. Every I/O operation specifies an explicit
//! byte offset. Because there is no shared mutable state, all I/O methods take
//! **`&self`**, enabling multiple operations to be dispatched concurrently from
//! different tasks on the same handle.
//!
//! Positional files are ideal when the access pattern is non-sequential: reading
//! scattered records from a database file, writing blocks to a pre-allocated
//! image, or serving range requests from a large static asset.
//!
//! | Type | Access | Obtained via |
//! |------|--------|-------------|
//! | [`ReadOnlyPositionalFile`]  | Positional read  | [`ReadOnlyPositionalFile::open`] |
//! | [`WriteOnlyPositionalFile`] | Positional write  | [`WriteOnlyPositionalFile::create`], [`WriteOnlyPositionalFile::create_new`] |
//! | [`PositionalFile`]          | Positional read + write | [`PositionalFile::open`], [`PositionalFile::create`], [`PositionalOpenOptions`] |
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use file::{ReadOnlyPositionalFile, Root};
//!
//! let dir = Root::bind("/var/data").await?;
//! let file = ReadOnlyPositionalFile::open(&dir, "db.bin").await?;
//!
//! // Read two disjoint regions concurrently — both calls use &self.
//! let (header, record) = tokio::join!(
//!     file.read_exact_at(0, 128),
//!     file.read_exact_at(4096, 256),
//! );
//! let header = header?;
//! let record = record?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Choosing between seekable and positional
//!
//! | Use case | Recommended type |
//! |----------|-----------------|
//! | Read a file from start to end | [`ReadOnlyFile`] |
//! | Append log entries | [`WriteOnlyFile`] |
//! | Build a file incrementally (write, then rewind and read) | [`File`] |
//! | Read scattered records from a database or index | [`ReadOnlyPositionalFile`] |
//! | Write blocks to a pre-allocated file | [`WriteOnlyPositionalFile`] |
//! | Serve concurrent range requests from a static asset | [`ReadOnlyPositionalFile`] |
//! | Read and update a memory-mapped-style structure | [`PositionalFile`] |
//!
//! ## Narrowing capabilities
//!
//! Both [`File`] and [`PositionalFile`] can be permanently narrowed to their
//! single-access counterparts via [`From`] conversions. Once narrowed, the
//! dropped capability cannot be recovered:
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use file::{File, ReadOnlyFile, Root};
//!
//! let dir = Root::bind("/var/data").await?;
//! let rw = File::open(&dir, "data.bin").await?;
//!
//! // Narrow to read-only — the write capability is permanently dropped.
//! let ro: ReadOnlyFile = rw.into();
//! # Ok(())
//! # }
//! ```
//!
//! # Buffer management
//!
//! All I/O uses buffers from the [`bytesbuf`] crate. [`BytesBuf`](bytesbuf::BytesBuf)
//! is a mutable write buffer; [`BytesView`](bytesbuf::BytesView) is an immutable,
//! reference-counted read view. Buffers are allocated from a memory provider
//! (defaulting to [`GlobalPool`](bytesbuf::mem::GlobalPool)).
//!
//! Each file type implements [`HasMemory`](bytesbuf::mem::HasMemory) and
//! [`Memory`](bytesbuf::mem::Memory), so you can reserve optimally-sized buffers
//! directly from the file:
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use bytesbuf::mem::Memory;
//! use file::{Root, WriteOnlyFile};
//!
//! let dir = Root::bind("/var/data").await?;
//! let mut file = WriteOnlyFile::create(&dir, "output.bin").await?;
//!
//! let mut buf = file.reserve(4096);
//! buf.put_slice(*b"Hello, world!");
//! file.write(buf.consume_all()).await?;
//! # Ok(())
//! # }
//! ```
//!
//! For zero-copy cross-subsystem transfers, constructors accept an optional custom
//! memory provider via `_with_memory` variants:
//!
//! ```ignore
//! // Open a file using the socket's memory provider.
//! let file = ReadOnlyFile::open_with_memory(&dir, "data.bin", socket.memory()).await?;
//!
//! // Data lands in memory optimal for the socket — zero copies on write.
//! let data = file.read_max(8192).await?;
//! socket.write(data).await?;
//! ```
//!
//! # Streaming I/O (seekable files)
//!
//! Seekable files support cursor-relative streaming. Use `read_max` to pull
//! data in chunks, or `write` / `write_slice` to push data sequentially:
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use bytesbuf::mem::Memory;
//! use file::{Root, WriteOnlyFile};
//!
//! let dir = Root::bind("/var/data").await?;
//! let mut file = WriteOnlyFile::create(&dir, "output.bin").await?;
//! for i in 0..10 {
//!     let mut buf = file.reserve(1024);
//!     buf.put_slice(*b"some data\n");
//!     file.write(buf.consume_all()).await?;
//! }
//! file.flush().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Callers working with plain `&[u8]` slices can use convenience methods like
//! [`WriteOnlyFile::write_slice`] and [`ReadOnlyFile::read_into_slice`]. Note
//! that these copy data internally; for large or performance-sensitive I/O,
//! prefer the [`BytesView`](bytesbuf::BytesView) methods.
//!
//! # Positional I/O (positional files)
//!
//! Positional files accept an explicit byte offset on every call. Because
//! they take `&self`, you can share a single handle across tasks:
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use file::{PositionalFile, Root};
//!
//! let dir = Root::bind("/var/data").await?;
//!
//! // Pre-allocate a 1 MB file, then write four 256 KB regions concurrently.
//! let file = PositionalFile::create(&dir, "image.bin").await?;
//! file.set_len(1_048_576).await?;
//!
//! let data = vec![0xABu8; 262_144];
//! let (a, b, c, d) = tokio::join!(
//!     file.write_slice_at(0,       &data),
//!     file.write_slice_at(262_144, &data),
//!     file.write_slice_at(524_288, &data),
//!     file.write_slice_at(786_432, &data),
//! );
//! a?; b?; c?; d?;
//! file.flush().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Positional files also offer `read_into_slice_at` and `write_slice_at` for
//! callers working with plain byte slices.

pub use std::fs::{FileTimes, FileType, Metadata, Permissions, TryLockError};
pub use std::io::SeekFrom;

pub use crate::dir_builder::DirBuilder;
pub use crate::dir_entry::DirEntry;
pub use crate::directory::Directory;
pub use crate::file::File;
pub use crate::open_options::OpenOptions;
pub use crate::positional_file::PositionalFile;
pub use crate::positional_open_options::PositionalOpenOptions;
pub use crate::read_dir::ReadDir;
pub use crate::read_only_file::ReadOnlyFile;
pub use crate::read_only_positional_file::ReadOnlyPositionalFile;
pub use crate::root::Root;
pub use crate::write_only_file::WriteOnlyFile;
pub use crate::write_only_positional_file::WriteOnlyPositionalFile;

mod dir_builder;
mod dir_entry;
mod directory;
mod dispatcher;
mod file;
mod file_inner;
mod open_options;
mod path_utils;
mod positional_file;
mod positional_open_options;
mod read_dir;
mod read_only_file;
mod read_only_positional_file;
mod root;
mod shared_memory;
mod write_only_file;
mod write_only_positional_file;
