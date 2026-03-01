<div align="center">
 <img src="./logo.png" alt="File Logo" width="96">

# File

[![crate.io](https://img.shields.io/crates/v/file.svg)](https://crates.io/crates/file)
[![docs.rs](https://docs.rs/file/badge.svg)](https://docs.rs/file)
[![MSRV](https://img.shields.io/crates/msrv/file)](https://crates.io/crates/file)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Zero-copy asynchronous filesystem API.

This crate provides a filesystem API that differs from [`std::fs`][__link0] in three key ways:

1. **Capability-based access control.** All filesystem operations are scoped to a
   [`Directory`][__link1] capability obtained via [`Root::bind`][__link2]. Paths are always relative
   to a directory, and path traversals that would escape the directory (such as
   leading `/` or `..` above the root) are rejected. This makes it possible to
   grant a subsystem access to a specific directory tree without risking access
   to the rest of the filesystem.

1. **Fully asynchronous.** Every I/O operation is `async`. The implementation uses
   a pool of dedicated background threads to perform blocking filesystem calls,
   keeping the async executor free.

1. **Managed buffers via [`bytesbuf`][__link3].** Reads produce
   [`BytesView`][__link4] values backed by pooled memory; writes
   accept them. This enables zero-copy data pipelines: data read from a file can
   be written to a socket (or another file) without intermediate copies, as long
   as both endpoints share a compatible memory provider.

## Quick start

```rust
use file::Root;

// Bind to a directory — the only place an absolute path is accepted.
let dir = Root::bind("/var/data").await?;

// Read and write whole files through the Directory capability.
dir.write_slice("greeting.txt", b"Hello!").await?;
let text = dir.read_to_string("greeting.txt").await?;

// Narrow the capability to a subdirectory.
let sub = dir.open_dir("subdir").await?;
let data = sub.read("nested_file.txt").await?;
```

## File types

The crate provides **six file types** organized into two families. Within each
family, three types enforce read, write, or read-write access at the type level.

### Seekable files — streaming I/O with a cursor

Seekable files maintain an internal cursor that advances with each read or
write. They implement [`bytesbuf_io::Read`][__link5] and/or [`bytesbuf_io::Write`][__link6] for
streaming I/O and support [`seek`][__link7],
[`stream_position`][__link8], and [`rewind`][__link9].

Because the cursor is shared mutable state, all I/O methods take **`&mut self`**,
ensuring only one operation is in flight at a time. This makes seekable files
ideal for sequential processing: reading a log from top to bottom, writing a
report line by line, or appending to a file.

|Type|Access|Obtained via|
|----|------|------------|
|[`ReadOnlyFile`][__link10]|Read + seek|[`ReadOnlyFile::open`][__link11]|
|[`WriteOnlyFile`][__link12]|Write + seek|[`WriteOnlyFile::create`][__link13], [`WriteOnlyFile::create_new`][__link14]|
|[`File`][__link15]|Read + write + seek|[`File::open`][__link16], [`File::create`][__link17], [`OpenOptions`][__link18]|

```rust
use file::{ReadOnlyFile, Root};

let dir = Root::bind("/var/data").await?;
let mut file = ReadOnlyFile::open(&dir, "log.txt").await?;

// Stream through the file in 8 KB chunks.
loop {
    let chunk = file.read_max(8192).await?;
    if chunk.is_empty() {
        break; // EOF
    }
    // process chunk...
}
```

### Positional files — offset-based I/O without a cursor

Positional files have **no cursor**. Every I/O operation specifies an explicit
byte offset. Because there is no shared mutable state, all I/O methods take
**`&self`**, enabling multiple operations to be dispatched concurrently from
different tasks on the same handle.

Positional files are ideal when the access pattern is non-sequential: reading
scattered records from a database file, writing blocks to a pre-allocated
image, or serving range requests from a large static asset.

|Type|Access|Obtained via|
|----|------|------------|
|[`ReadOnlyPositionalFile`][__link19]|Positional read|[`ReadOnlyPositionalFile::open`][__link20]|
|[`WriteOnlyPositionalFile`][__link21]|Positional write|[`WriteOnlyPositionalFile::create`][__link22], [`WriteOnlyPositionalFile::create_new`][__link23]|
|[`PositionalFile`][__link24]|Positional read + write|[`PositionalFile::open`][__link25], [`PositionalFile::create`][__link26], [`PositionalOpenOptions`][__link27]|

```rust
use file::{ReadOnlyPositionalFile, Root};

let dir = Root::bind("/var/data").await?;
let file = ReadOnlyPositionalFile::open(&dir, "db.bin").await?;

// Read two disjoint regions concurrently — both calls use &self.
let (header, record) = tokio::join!(
    file.read_exact_at(0, 128),
    file.read_exact_at(4096, 256),
);
let header = header?;
let record = record?;
```

### Choosing between seekable and positional

|Use case|Recommended type|
|--------|----------------|
|Read a file from start to end|[`ReadOnlyFile`][__link28]|
|Append log entries|[`WriteOnlyFile`][__link29]|
|Build a file incrementally (write, then rewind and read)|[`File`][__link30]|
|Read scattered records from a database or index|[`ReadOnlyPositionalFile`][__link31]|
|Write blocks to a pre-allocated file|[`WriteOnlyPositionalFile`][__link32]|
|Serve concurrent range requests from a static asset|[`ReadOnlyPositionalFile`][__link33]|
|Read and update a memory-mapped-style structure|[`PositionalFile`][__link34]|

### Narrowing capabilities

Both [`File`][__link35] and [`PositionalFile`][__link36] can be permanently narrowed to their
single-access counterparts via [`From`][__link37] conversions. Once narrowed, the
dropped capability cannot be recovered:

```rust
use file::{File, ReadOnlyFile, Root};

let dir = Root::bind("/var/data").await?;
let rw = File::open(&dir, "data.bin").await?;

// Narrow to read-only — the write capability is permanently dropped.
let ro: ReadOnlyFile = rw.into();
```

## Buffer management

All I/O uses buffers from the [`bytesbuf`][__link38] crate. [`BytesBuf`][__link39]
is a mutable write buffer; [`BytesView`][__link40] is an immutable,
reference-counted read view. Buffers are allocated from a memory provider
(defaulting to [`GlobalPool`][__link41]).

Each file type implements [`HasMemory`][__link42] and
[`Memory`][__link43], so you can reserve optimally-sized buffers
directly from the file:

```rust
use bytesbuf::mem::Memory;
use file::{Root, WriteOnlyFile};

let dir = Root::bind("/var/data").await?;
let mut file = WriteOnlyFile::create(&dir, "output.bin").await?;

let mut buf = file.reserve(4096);
buf.put_slice(*b"Hello, world!");
file.write(buf.consume_all()).await?;
```

For zero-copy cross-subsystem transfers, constructors accept an optional custom
memory provider via `_with_memory` variants:

```rust
// Open a file using the socket's memory provider.
let file = ReadOnlyFile::open_with_memory(&dir, "data.bin", socket.memory()).await?;

// Data lands in memory optimal for the socket — zero copies on write.
let data = file.read_max(8192).await?;
socket.write(data).await?;
```

## Streaming I/O (seekable files)

Seekable files support cursor-relative streaming. Use `read_max` to pull
data in chunks, or `write` / `write_slice` to push data sequentially:

```rust
use bytesbuf::mem::Memory;
use file::{Root, WriteOnlyFile};

let dir = Root::bind("/var/data").await?;
let mut file = WriteOnlyFile::create(&dir, "output.bin").await?;
for i in 0..10 {
    let mut buf = file.reserve(1024);
    buf.put_slice(*b"some data\n");
    file.write(buf.consume_all()).await?;
}
file.flush().await?;
```

Callers working with plain `&[u8]` slices can use convenience methods like
[`WriteOnlyFile::write_slice`][__link44] and [`ReadOnlyFile::read_into_slice`][__link45]. Note
that these copy data internally; for large or performance-sensitive I/O,
prefer the [`BytesView`][__link46] methods.

## Positional I/O (positional files)

Positional files accept an explicit byte offset on every call. Because
they take `&self`, you can share a single handle across tasks:

```rust
use file::{PositionalFile, Root};

let dir = Root::bind("/var/data").await?;

// Pre-allocate a 1 MB file, then write four 256 KB regions concurrently.
let file = PositionalFile::create(&dir, "image.bin").await?;
file.set_len(1_048_576).await?;

let data = vec![0xABu8; 262_144];
let (a, b, c, d) = tokio::join!(
    file.write_slice_at(0,       &data),
    file.write_slice_at(262_144, &data),
    file.write_slice_at(524_288, &data),
    file.write_slice_at(786_432, &data),
);
a?; b?; c?; d?;
file.flush().await?;
```

Positional files also offer `read_into_slice_at` and `write_slice_at` for
callers working with plain byte slices.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/file">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG2q7uLyplaUVG_BaW6a77dCzGyul8moi2JnDG_GS_QHWwc9xYWSCgmhieXRlc2J1ZmUwLjQuMIJrYnl0ZXNidWZfaW9lMC40LjA
 [__link0]: https://doc.rust-lang.org/stable/std/?search=fs
 [__link1]: https://doc.rust-lang.org/stable/std/?search=file::directory::Directory
 [__link10]: https://doc.rust-lang.org/stable/std/?search=file::read_only_file::ReadOnlyFile
 [__link11]: https://doc.rust-lang.org/stable/std/?search=file::read_only_file::ReadOnlyFile::open
 [__link12]: https://doc.rust-lang.org/stable/std/?search=file::write_only_file::WriteOnlyFile
 [__link13]: https://doc.rust-lang.org/stable/std/?search=file::write_only_file::WriteOnlyFile::create
 [__link14]: https://doc.rust-lang.org/stable/std/?search=file::write_only_file::WriteOnlyFile::create_new
 [__link15]: https://doc.rust-lang.org/stable/std/?search=file::file::File
 [__link16]: https://doc.rust-lang.org/stable/std/?search=file::file::File::open
 [__link17]: https://doc.rust-lang.org/stable/std/?search=file::file::File::create
 [__link18]: https://doc.rust-lang.org/stable/std/?search=file::open_options::OpenOptions
 [__link19]: https://doc.rust-lang.org/stable/std/?search=file::read_only_positional_file::ReadOnlyPositionalFile
 [__link2]: https://doc.rust-lang.org/stable/std/?search=file::root::Root::bind
 [__link20]: https://doc.rust-lang.org/stable/std/?search=file::read_only_positional_file::ReadOnlyPositionalFile::open
 [__link21]: https://doc.rust-lang.org/stable/std/?search=file::write_only_positional_file::WriteOnlyPositionalFile
 [__link22]: https://doc.rust-lang.org/stable/std/?search=file::write_only_positional_file::WriteOnlyPositionalFile::create
 [__link23]: https://doc.rust-lang.org/stable/std/?search=file::write_only_positional_file::WriteOnlyPositionalFile::create_new
 [__link24]: https://doc.rust-lang.org/stable/std/?search=file::positional_file::PositionalFile
 [__link25]: https://doc.rust-lang.org/stable/std/?search=file::positional_file::PositionalFile::open
 [__link26]: https://doc.rust-lang.org/stable/std/?search=file::positional_file::PositionalFile::create
 [__link27]: https://doc.rust-lang.org/stable/std/?search=file::positional_open_options::PositionalOpenOptions
 [__link28]: https://doc.rust-lang.org/stable/std/?search=file::read_only_file::ReadOnlyFile
 [__link29]: https://doc.rust-lang.org/stable/std/?search=file::write_only_file::WriteOnlyFile
 [__link3]: https://crates.io/crates/bytesbuf/0.4.0
 [__link30]: https://doc.rust-lang.org/stable/std/?search=file::file::File
 [__link31]: https://doc.rust-lang.org/stable/std/?search=file::read_only_positional_file::ReadOnlyPositionalFile
 [__link32]: https://doc.rust-lang.org/stable/std/?search=file::write_only_positional_file::WriteOnlyPositionalFile
 [__link33]: https://doc.rust-lang.org/stable/std/?search=file::read_only_positional_file::ReadOnlyPositionalFile
 [__link34]: https://doc.rust-lang.org/stable/std/?search=file::positional_file::PositionalFile
 [__link35]: https://doc.rust-lang.org/stable/std/?search=file::file::File
 [__link36]: https://doc.rust-lang.org/stable/std/?search=file::positional_file::PositionalFile
 [__link37]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
 [__link38]: https://crates.io/crates/bytesbuf/0.4.0
 [__link39]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=BytesBuf
 [__link4]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=BytesView
 [__link40]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=BytesView
 [__link41]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=mem::GlobalPool
 [__link42]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=mem::HasMemory
 [__link43]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=mem::Memory
 [__link44]: https://doc.rust-lang.org/stable/std/?search=file::write_only_file::WriteOnlyFile::write_slice
 [__link45]: https://doc.rust-lang.org/stable/std/?search=file::read_only_file::ReadOnlyFile::read_into_slice
 [__link46]: https://docs.rs/bytesbuf/0.4.0/bytesbuf/?search=BytesView
 [__link5]: https://docs.rs/bytesbuf_io/0.4.0/bytesbuf_io/?search=Read
 [__link6]: https://docs.rs/bytesbuf_io/0.4.0/bytesbuf_io/?search=Write
 [__link7]: https://doc.rust-lang.org/stable/std/?search=file::file::File::seek
 [__link8]: https://doc.rust-lang.org/stable/std/?search=file::file::File::stream_position
 [__link9]: https://doc.rust-lang.org/stable/std/?search=file::file::File::rewind
