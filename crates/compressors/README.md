<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Compressors Logo" width="96">

# Compressors

[![crate.io](https://img.shields.io/crates/v/compressors.svg)](https://crates.io/crates/compressors)
[![docs.rs](https://docs.rs/compressors/badge.svg)](https://docs.rs/compressors)
[![MSRV](https://img.shields.io/crates/msrv/compressors)](https://crates.io/crates/compressors)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Streaming compression and decompression over [`bytesbuf`][__link0] byte sequences.

Each supported format – `deflate`, `zlib`, `gzip`, `brotli`, `zstd` – lives in a module of its
own behind a cargo feature of its own. What those modules share is uniform: `compress`,
`decompress`, `Compressor` and `Decompressor` have the same shape in every one of them, so
moving a call site between formats is a change of import. Their builders are not uniform:
`brotli` and `zstd` add format-specific settings, and their compressor `build` returns a
[`Result`][__link1], so switching a builder call site can take more than an import change.

**Engine** below means a third-party format implementation (`flate2`, the `brotli` crate,
`zstd-safe`) together with the working memory it allocates. A `Compressor` or a `Decompressor`
owns one, configured and positioned in a single stream, and returns it to [`Resources`][__link2] on drop.

This crate is distinguished by:

* **It reads and writes [`bytesbuf`][__link3] sequences directly.** Input is read from a [`BytesView`][__link4]’s
  segments where they already sit, and output is written into the uninitialized spare capacity
  of a [`BytesBuf`][__link5]. Nothing is flattened into an intermediate buffer on the
  way in, and nothing is copied out of one on the way back.
* **It recycles engine state.** [`Resources`][__link6] keeps the window and hash tables an engine
  allocates and hands them to the next compressor or decompressor that needs them. On a small
  message that setup costs about as much as the compression itself, so the saving is worth
  having.
* **One API spans every format, at any size.** The same push/pull contract drives every engine,
  so code is written once and works with whichever one it is given. Because an engine is a state
  machine rather than a one-shot transform, a stream of any length passes through it while the
  pending output it buffers stays bounded by the configured chunk size.

Secondarily, this is also why the engines are not driven through `std::io`. That route works –
[`BytesView`][__link7] implements `BufRead` over its segments and `BytesBufWriter` implements `Write`
into segmented storage, so nothing has to be flattened to use it. What the direct adapters buy
is narrower: output goes straight into a [`BytesBuf`][__link8]’s uninitialized spare
capacity rather than through an intermediate buffer the adapter owns, engine state stays
reusable from one stream to the next, and flush and chunk boundaries remain under this crate’s
control.

## Whole buffers

Each format module has its own `compress` and `decompress` for the common case. The crate-level
[`compress`][__link9] and [`decompress`][__link10] instead accept any engine implementing [`Compression`][__link11],
however it was constructed.

```rust
use compressors::{Resources, gzip};

let resources = Resources::global();
let compressed = gzip::compress(b"hello", resources)?;

assert_eq!(
    gzip::decompress(compressed, resources)?.to_vec(),
    b"hello".to_vec()
);
```

## Streaming

An engine is a state machine rather than a one-shot transform, so a stream of any length moves
through it while the output it has buffered but not yet handed back stays bounded by the
configured chunk size. Pending input and the engine’s own window and tables are additional, and
their size depends on the format and its configuration. [`CompressionStream`][__link12], behind the
`futures-stream` feature, is how to reach that – it turns any stream of byte sequences into its
compressed or decompressed counterpart:

```rust
use std::io::Error as IoError;

use bytesbuf::BytesView;
use compressors::{CompressionStream, Resources, gzip};
use futures::{TryStreamExt, stream};

let resources = Resources::global();
let body = stream::iter(vec![
    Ok::<_, IoError>(BytesView::copied_from_slice(b"a body ", resources.memory())),
    Ok(BytesView::copied_from_slice(
        b"in pieces",
        resources.memory(),
    )),
]);

let mut compressed = CompressionStream::compress(body, gzip::Compressor::new(resources));

// Each chunk is inspected and dropped as it arrives, so the caller stays bounded too --
// collecting them all would put the whole encoded body back in memory.
let mut magic = Vec::new();
while let Some(chunk) = compressed.try_next().await? {
    if magic.is_empty() && chunk.len() >= 2 {
        magic = chunk.range(0..2).to_vec();
    }
}

assert_eq!(magic, vec![0x1f, 0x8b]);
```

## Choosing a format

When the format is only known at runtime – from a `Content-Encoding` token, say – the
[`format`][__link13] module resolves the token and carries the same shape every other
format module does: a `Compressor`, a `Decompressor`, and the whole-buffer conveniences. Use
[`CompressorBuilder::build_format`][__link14] instead when a level or chunk size has to be set on the
result.

Note that the `deflate` feature and the HTTP `deflate` content coding are not the same thing.
`Format::Deflate` is raw DEFLATE (RFC 1951), which has no content-coding token, so
`Format::Deflate.content_encoding()` returns `None`. The HTTP `deflate` token denotes a
zlib-wrapped stream (RFC 1950), so `Format::from_content_encoding("deflate")` resolves to
`Format::Zlib` and needs the `zlib` feature, not the `deflate` one.

```rust
use compressors::Resources;
use compressors::format::{self, Format};

let format = Format::from_content_encoding("gzip").expect("this build supports gzip");

let resources = Resources::global();
let compressed = format::compress(format, b"runtime selected", resources)?;

assert_eq!(
    format::decompress(format, compressed, resources)?.to_vec(),
    b"runtime selected".to_vec()
);
```

## Reusing engine state

Building a compressor allocates and initializes a substantial amount of state – on a small
message, as much work as the compression itself. [`Resources`][__link15] recycles it: hold one, hand it to
every compressor and decompressor, and each engine returns to it on drop. The saving is roughly
fixed per message, so it matters most for small bodies.

Recycling is on by default, which is why every API that builds an engine asks for resources rather
than for a memory provider alone. Set the capacity to zero with
[`enable_pooling`][__link16] when compression is rare enough that retaining
engine state costs more than rebuilding it.

```rust
use compressors::{Level, Resources, gzip};

// Held once by the application, cloned into whatever needs it.
let resources = Resources::global();

// Per request: cheap to build, recycles the engine on drop.
let compressor = gzip::Compressor::builder()
    .level(Level::DEFAULT)
    .build(resources);
```

Recycling applies only to the engines whose state is expensive enough to be worth retaining and
is skipped for the rest, so calling code never has to know which engines benefit.

## Security

Every one of these formats can expand its input by orders of magnitude, so a decompressor
pointed at untrusted data is a memory-exhaustion vector. A decompressor driven directly never
accumulates – each chunk it hands back is bounded – so the exposure is in what the caller
keeps, which makes it the conveniences that buffer a whole result that need bounding. Those add
a 64 MiB output cap and a 1024 concatenated-stream cap to whatever the caller did not set.

When you buffer decompressed output yourself, set
[`with_max_output_len`][__link17] to what you can afford. That
guardrail is for the common case, not a substitute for bounding how many bodies you decompress
at once. [`DecompressorLimits`][__link18] documents what each format bounds by default, and why a ratio
alone is not protection.

Decompression can yield bytes before a checksum or trailer has rejected the stream, so treat
them as provisional until the decompressor reports that it is done.

## Features

Every format is a separate feature and none is on by default, so a build compiles only the
engines it names:

* `gzip` – the `gzip` module and `Format::Gzip`, via `flate2`. Accepted by essentially every
  HTTP client and server, so it is the safe default when the peer’s capabilities are unknown.
* `deflate` – the `deflate` module and `Format::Deflate`, via `flate2`. Raw DEFLATE, with no
  HTTP content-coding token of its own.
* `zlib` – the `zlib` module and `Format::Zlib`, via `flate2`. This is what the HTTP `deflate`
  content coding actually denotes.
* `brotli` – the `brotli` module and `Format::Brotli`, via the pure-Rust `brotli` crate.
* `zstd` – the `zstd` module and `Format::Zstd`, via `zstd-safe`.
* `futures-stream` – [`CompressionStream`][__link19], presenting compression and decompression as a
  `futures_core::Stream` over any stream of byte sequences.

The deflate-family features share one dependency, so enabling more than one of them costs no
more than enabling one. A build that needs only `brotli` or only `zstd` never compiles `flate2`
at all, and a build that names no format at all still gets [`Compression`][__link20], the builders and
[`Resources`][__link21], which is what a crate that only passes compressors and decompressors around
needs.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/compressors">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb1zOULLzYMFUbWNLhT9xgsUwbQyNc6rGvTwcbZLzN8FpQrBphZIKCaGJ5dGVzYnVmZTAuOS4wgmtjb21wcmVzc29yc2UwLjEuMA
 [__link0]: https://crates.io/crates/bytesbuf/0.9.0
 [__link1]: https://docs.rs/compressors/0.1.0/compressors/?search=Result
 [__link10]: https://docs.rs/compressors/0.1.0/compressors/fn.decompress.html
 [__link11]: https://docs.rs/compressors/0.1.0/compressors/?search=core::Compression
 [__link12]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressionStream
 [__link13]: mod@crate::format
 [__link14]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressorBuilder::build_format
 [__link15]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link16]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources::enable_pooling
 [__link17]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressorLimits::with_max_output_len
 [__link18]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressorLimits
 [__link19]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressionStream
 [__link2]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link20]: https://docs.rs/compressors/0.1.0/compressors/?search=core::Compression
 [__link21]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link3]: https://crates.io/crates/bytesbuf/0.9.0
 [__link4]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesView
 [__link5]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesBuf
 [__link6]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link7]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesView
 [__link8]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesBuf
 [__link9]: https://docs.rs/compressors/0.1.0/compressors/fn.compress.html
