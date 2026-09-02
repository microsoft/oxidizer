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

Five formats are available, each behind a cargo feature of its own: `deflate`, `zlib`,
`gzip`, `brotli` and `zstd`. Each lives in its own module and exposes the same handful of items,
so moving between them is a change of import rather than a change of code.

Compression engines normally speak `std::io::Read` and `std::io::Write`, which assume a single
contiguous `&[u8]`. A [`BytesView`][__link1] is a chain of segments with no
contiguous representation, so bridging the two through `std::io` would mean copying every byte
into a flat buffer first. This crate drives the engine from the view’s segments directly, and
writes into the uninitialized spare capacity of a [`BytesBuf`][__link2], so no
intermediate copy is needed.

## Whole buffers

Each format module has its own `compress` and `decompress` for the common case. The crate-level
[`compress`][__link3] and [`decompress`][__link4] take an operation you already have instead, whatever built it.

```rust
use bytesbuf::BytesView;
use compressors::{Resources, gzip};

let resources = Resources::global();
let compressed = gzip::compress(
    BytesView::copied_from_slice(b"hello", resources.memory()),
    resources,
)?;

assert_eq!(
    gzip::decompress(compressed, resources)?.to_vec(),
    b"hello".to_vec()
);
```

## Streaming

A codec is a state machine rather than a one-shot transform, so a stream of any length moves
through it with a bounded working set: one pending input view and one output chunk, however many
gigabytes pass through. [`CompressionStream`][__link5], behind the `futures-stream` feature, is how to
reach that – it turns any stream of byte sequences into its compressed or decompressed
counterpart:

```rust
use bytesbuf::BytesView;
use compressors::{CompressionStream, Resources, gzip};
use futures::{StreamExt, stream};

let resources = Resources::global();
let body = stream::iter(vec![
    Ok::<_, std::io::Error>(BytesView::copied_from_slice(b"a body ", resources.memory())),
    Ok(BytesView::copied_from_slice(
        b"in pieces",
        resources.memory(),
    )),
]);

let chunks: Vec<_> = CompressionStream::compress(body, gzip::Compressor::new(resources))
    .collect()
    .await;

let gzip = BytesView::from_views(chunks.into_iter().map(|chunk| chunk.unwrap()));
assert_eq!(gzip.range(0..2).to_vec(), vec![0x1f, 0x8b]);
```

## Choosing a format

When the format is only known at runtime – from a `Content-Encoding` token, say – [`Format`][__link6]
resolves the token and compresses with whatever it names. Reach for
[`CompressorBuilder::build_format`][__link7] instead when the level or the chunk size matters: it returns
an operation that fits wherever a concrete one does.

```rust
use bytesbuf::BytesView;
use compressors::{Format, Resources};

let format = Format::from_content_encoding("gzip").expect("this build supports gzip");

let resources = Resources::global();
let compressed = format.compress(
    BytesView::copied_from_slice(b"runtime selected", resources.memory()),
    resources,
)?;

assert_eq!(
    format.decompress(compressed, resources)?.to_vec(),
    b"runtime selected".to_vec()
);
```

## Reusing engine state

Building a compressor allocates and initializes a substantial amount of state – on a small
message, as much work as the compression itself. [`Resources`][__link8] recycles it: hold one, hand it to
every operation, and each engine returns to it when its codec drops. The saving is roughly fixed
per message, so it matters most for small bodies.

Recycling is on by default, which is why every API that builds a codec asks for resources rather
than for a memory provider alone. Turn it off with
[`enable_pooling(0)`][__link9] when there is genuinely nothing to reuse.

```rust
use compressors::{Level, Resources, gzip};

// Held once by the application, cloned into whatever needs it.
let resources = Resources::global();

// Per request: cheap to build, recycles the engine on drop.
let compressor = gzip::Compressor::builder()
    .level(Level::DEFAULT)
    .build(resources);
```

Recycling is transparent – it applies to the engines that are worth it and quietly skips the
rest – so calling code never has to know which engines benefit.

## Security

Every one of these formats can expand its input by orders of magnitude, so a decompressor
pointed at untrusted data is a memory-exhaustion vector. Nothing here accumulates – each chunk a
codec hands back is bounded – so the exposure is in what the caller keeps, which makes it the
conveniences that buffer a whole result that need bounding.

For untrusted input use each format’s `decompress_with_limits`, or
[`Format::decompress_with_limits`][__link10], and set
[`with_max_output_len`][__link11] to what you can afford to
buffer. [`DecompressorLimits`][__link12] documents what each format bounds by default, and why a ratio
alone is not protection.

Decompression can yield bytes before a checksum or trailer has rejected the stream, so treat
them as provisional until the operation reports that it is done.

## Features

Every format is a separate feature and none is on by default, so a build compiles only the
engines it names:

* `gzip` – the `gzip` module and `Format::Gzip`, via `flate2`. The encoding most often seen on
  the wire, and the one to reach for when in doubt.
* `deflate` – the `deflate` module and `Format::Deflate`, via `flate2`.
* `zlib` – the `zlib` module and `Format::Zlib`, via `flate2`.
* `brotli` – the `brotli` module and `Format::Brotli`, via the pure-Rust `brotli` crate.
* `zstd` – the `zstd` module and `Format::Zstd`, via `zstd-safe`.
* `futures-stream` – [`CompressionStream`][__link13], presenting compression and decompression as a
  `futures_core::Stream` over any stream of byte sequences.

The deflate-family features share one dependency, so enabling all three costs no more than one.
A build that needs only `brotli` or only `zstd` never compiles `flate2` at all, and a build that
names no format at all still gets [`Compression`][__link14], the builders and [`Resources`][__link15], which is what
a crate that only passes operations around needs.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/compressors">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbKOT2sSODAQAbOy_IdbOlhRgbwyzwlvIC9Tkb9k-vnEIf9tRhZIKCaGJ5dGVzYnVmZTAuOS4wgmtjb21wcmVzc29yc2UwLjEuMA
 [__link0]: https://crates.io/crates/bytesbuf/0.9.0
 [__link1]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesView
 [__link10]: https://docs.rs/compressors/0.1.0/compressors/?search=Format::decompress_with_limits
 [__link11]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressorLimits::with_max_output_len
 [__link12]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressorLimits
 [__link13]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressionStream
 [__link14]: https://docs.rs/compressors/0.1.0/compressors/?search=core::Compression
 [__link15]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link2]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesBuf
 [__link3]: https://docs.rs/compressors/0.1.0/compressors/fn.compress.html
 [__link4]: https://docs.rs/compressors/0.1.0/compressors/fn.decompress.html
 [__link5]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressionStream
 [__link6]: https://docs.rs/compressors/0.1.0/compressors/?search=Format
 [__link7]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressorBuilder::build_format
 [__link8]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link9]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources::enable_pooling
