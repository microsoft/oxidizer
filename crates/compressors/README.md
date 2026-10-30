<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Compressors Logo" width="96">

# Compressors

[![crate.io](https://img.shields.io/crates/v/compressors.svg)](https://crates.io/crates/compressors)
[![docs.rs](https://docs.rs/compressors/badge.svg)](https://docs.rs/compressors)
[![MSRV](https://img.shields.io/crates/msrv/compressors)](https://crates.io/crates/compressors)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Streams compression and decompression over [`bytesbuf`][__link0] byte sequences.

Each supported format has a module and a Cargo feature with the same name. The formats are
`deflate`, `zlib`, `gzip`, `brotli`, and `zstd`. Each module provides `compress`, `decompress`,
`Compressor`, and `Decompressor`. The `brotli` and `zstd` builders also provide format-specific
settings. The zstd compressor and decompressor `build` methods return a [`Result`][__link1].

An engine is a third-party format implementation and its working memory. The implementations are
`flate2`, the `brotli` crate, and `zstd-safe`. A `Compressor` or `Decompressor` owns one engine
for one operation. When dropped, it returns reusable engine state to [`Resources`][__link2] if that
engine supports recycling.

This crate provides these capabilities:

* It reads existing [`BytesView`][__link3] segments directly.
* It writes output into the uninitialized spare capacity of a
  [`BytesBuf`][__link4].
* It recycles engine state through [`Resources`][__link5].
* It uses one push/pull contract for every format and stream length.

The engines do not use `std::io` adapters. Direct adapters write into the destination
[`BytesBuf`][__link6] without an intermediate output buffer. They also preserve
reusable engine state and keep flush and chunk boundaries under this crate’s control.

## Whole buffers

Each format module provides `compress` and `decompress`. The crate-level [`compress`][__link7] and
[`decompress`][__link8] functions accept any engine that implements [`Compression`][__link9].

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

An engine is a state machine. Its pending output stays bounded by the configured chunk size.
Pending input, the engine window, and engine tables use additional memory. Their size depends on
the format and its configuration. With the `futures-stream` feature, `CompressionStream`
converts a stream of byte sequences:

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

// Inspect and drop each chunk to keep retained memory bounded.
let mut magic = Vec::new();
while let Some(chunk) = compressed.try_next().await? {
    if magic.is_empty() && chunk.len() >= 2 {
        magic = chunk.range(0..2).to_vec();
    }
}

assert_eq!(magic, vec![0x1f, 0x8b]);
```

## Choosing a format

If the format is known only at run time, use the [`format` module][__link10].
It resolves a `Content-Encoding` token and provides `Compressor`, `Decompressor`, and
whole-buffer functions. Use [`CompressorBuilder::build_format`][__link11] to set a level or chunk size.

The `deflate` feature and the HTTP `deflate` content coding are different. `Format::Deflate` is
raw DEFLATE (RFC 1951), which has no content-coding token. Therefore,
`Format::Deflate.content_encoding()` returns `None`. The HTTP `deflate` token denotes a
zlib-wrapped stream (RFC 1950). `Format::from_content_encoding("deflate")` therefore resolves
to `Format::Zlib` and needs the `zlib` feature, not the `deflate` feature.

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

Building a compressor allocates and initializes engine state. For a small message, this setup
costs about as much as compression. [`Resources`][__link12] recycles this state. Keep one value and pass it
to each compressor and decompressor.

Recycling is enabled by default. Each API that builds an engine therefore takes `Resources`.
Set the pool capacity to zero with [`with_pool_capacity`][__link13] to
disable recycling.

```rust
use compressors::{Level, Resources, gzip};

// Keep one value for the application.
let resources = Resources::global();

// Build one compressor for each request. Dropping it recycles its engine.
let compressor = gzip::Compressor::builder()
    .level(Level::DEFAULT)
    .build(resources);
```

The pool retains only engine state for which reuse justifies its memory cost. This choice is not
part of the public contract.

## Security

Each format can expand its input by orders of magnitude. Untrusted compressed data can exhaust
memory. `CompressionStream` does not accumulate output, but its caller can still retain chunks.
Whole-buffer decompression adds fallback limits for bounds that the caller did not set. The
fallbacks are 64 MiB of output and 1024 concatenated streams.

If you buffer decompressed output, set
[`max_output_len`][__link14] to an affordable value. Also limit the
number of operations that decompress data at the same time. [`DecompressorLimits`][__link15] lists the
defaults for each format.

Decompression can yield bytes before a checksum or trailer has rejected the stream, so treat
them as provisional until the decompressor reports that it is done. A checksum match is not an
authenticity claim. These formats are not authenticated. Decompression does not make content
trusted.

See [`documentation`][__link16] for the full threat model.

## Features

Each format is a separate feature. No format is enabled by default:

* `gzip`: The `gzip` module and `Format::Gzip`, through `flate2`. Most HTTP clients and servers
  accept this format.
* `deflate`: The `deflate` module and `Format::Deflate`, through `flate2`. Raw DEFLATE has no
  HTTP content-coding token of its own.
* `zlib`: The `zlib` module and `Format::Zlib`, through `flate2`. The HTTP `deflate` content
  coding denotes this format.
* `brotli`: The `brotli` module and `Format::Brotli`, through the pure-Rust `brotli` crate.
* `zstd`: The `zstd` module and `Format::Zstd`, through `zstd-safe`.
* `futures-stream`: `CompressionStream` presents compression and decompression as a
  `futures_core::Stream` over any stream of byte sequences.

The deflate-family features share one dependency. A build that needs only `brotli` or `zstd`
does not compile `flate2`. A build with no format still provides [`Compression`][__link17], the builders,
and [`Resources`][__link18].

## Further reading

See [`documentation`][__link19] for the longer form guides.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/compressors">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbiqZQXhcFxiEbzy3RkWnWzqwbelDYub1Z6pYbQDIZIKQR3j1hZIKCaGJ5dGVzYnVmZjAuMTIuMIJrY29tcHJlc3NvcnNlMC4zLjA
 [__link0]: https://crates.io/crates/bytesbuf/0.12.0
 [__link1]: https://docs.rs/compressors/0.3.0/compressors/?search=Result
 [__link10]: https://docs.rs/compressors/0.3.0/compressors/?search=format
 [__link11]: https://docs.rs/compressors/0.3.0/compressors/?search=CompressorBuilder::build_format
 [__link12]: https://docs.rs/compressors/0.3.0/compressors/?search=Resources
 [__link13]: https://docs.rs/compressors/0.3.0/compressors/?search=Resources::with_pool_capacity
 [__link14]: https://docs.rs/compressors/0.3.0/compressors/?search=DecompressorLimits::max_output_len
 [__link15]: https://docs.rs/compressors/0.3.0/compressors/?search=DecompressorLimits
 [__link16]: https://docs.rs/compressors/0.3.0/compressors/documentation/index.html
 [__link17]: https://docs.rs/compressors/0.3.0/compressors/?search=core::Compression
 [__link18]: https://docs.rs/compressors/0.3.0/compressors/?search=Resources
 [__link19]: https://docs.rs/compressors/0.3.0/compressors/documentation/index.html
 [__link2]: https://docs.rs/compressors/0.3.0/compressors/?search=Resources
 [__link3]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesView
 [__link4]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesBuf
 [__link5]: https://docs.rs/compressors/0.3.0/compressors/?search=Resources
 [__link6]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesBuf
 [__link7]: https://docs.rs/compressors/0.3.0/compressors/fn.compress.html
 [__link8]: https://docs.rs/compressors/0.3.0/compressors/fn.decompress.html
 [__link9]: https://docs.rs/compressors/0.3.0/compressors/?search=core::Compression
