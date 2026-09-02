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
`gzip`, `brotli` and `zstd`. Each lives in its own module and exposes the same seven items,
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
use bytesbuf::mem::GlobalPool;
use compressors::{Resources, gzip};

let memory = GlobalPool::new();
let compressed = gzip::compress(
    BytesView::copied_from_slice(b"hello", &memory),
    &Resources::default(),
)?;

assert_eq!(
    gzip::decompress(compressed, &Resources::default())?.to_vec(),
    b"hello".to_vec()
);
```

## Streaming

[`gzip::Compressor`][__link5] and [`gzip::Decompressor`][__link6] are push/pull state machines rather than one-shot
transforms. They carry no operations of their own: everything is driven through
[`Compression`][__link7], so the same loop works for any format. Each `pull` returns at most one chunk,
so processing a multi-gigabyte stream never holds more than one pending input view plus one
output chunk:

```rust
use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use compressors::core::Compression;
use compressors::{Output, Resources, gzip};

let mut decompressor = gzip::Decompressor::new(&Resources::default());
let mut chunks = source.into_iter();
let mut plain = BytesBuf::new();

loop {
    match decompressor.pull()? {
        Output::Data(data) => plain.put_bytes(data),
        Output::Progress => {}
        Output::NeedInput => match chunks.next() {
            Some(chunk) => decompressor.push(chunk)?,
            None => decompressor.end_input(),
        },
        Output::Done => break,
    }
}

assert_eq!(plain.consume_all().to_vec(), b"streamed".to_vec());
```

## Choosing a format

The [`Compression`][__link8] trait describes the contract independently of the format and direction, so
code can be written once and used with any implementation. When the format is only known at
runtime – from a `Content-Encoding` token, say – [`Format`][__link9] resolves it, and
[`CompressorBuilder::build_format`][__link10] produces a boxed operation, which is itself a `Compression`
and so fits anywhere a concrete one does:

```rust
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::Resources;
use compressors::Format;

let format = Format::from_content_encoding("gzip").expect("this build supports gzip");

let memory = GlobalPool::new();
let compressed = format.compress(
    BytesView::copied_from_slice(b"runtime selected", &memory),
    &Resources::default(),
)?;

assert_eq!(
    format.decompress(compressed, &Resources::default())?.to_vec(),
    b"runtime selected".to_vec()
);
```

## Reusing engine state

Building a compressor allocates and initializes a substantial amount of state – on a small
message, as much work as the compression itself. [`Resources`][__link11] recycles it: hold one, hand it to
every operation, and each engine returns to it when its codec drops. The saving is roughly fixed
per message, so it matters most for small bodies.

Recycling is on by default, which is why every API that builds a codec asks for resources rather
than for a memory provider alone. Turn it off with
[`enable_pooling(0)`][__link12] when there is genuinely nothing to reuse.

```rust
use compressors::{Level, Resources, gzip};

// Held once by the application, cloned into whatever needs it.
let resources = Resources::global();

// Per request: cheap to build, recycles the engine on drop.
let compressor = gzip::Compressor::builder().level(Level::DEFAULT).build(resources);
```

Recycling is transparent – it applies to the engines that are worth it and quietly skips the
rest – so calling code never has to know which engines benefit.

## Security

Every one of these formats can expand its input by orders of magnitude, so a decompressor pointed at
untrusted data is a memory-exhaustion vector.

The codecs themselves never accumulate: each `pull` hands back one bounded chunk, so nothing in
this crate grows with the length of the stream. The exposure belongs to whatever the caller does
with those chunks, which is why the limits matter most for the accumulating conveniences –
`compress`, `decompress`, and [`Format::compress`][__link13] / [`Format::decompress`][__link14].
Use each format’s `decompress_with_limits` or [`Format::decompress_with_limits`][__link15] for
untrusted in-memory input.

Each format declares its own default bounds, because a single portable ratio cannot serve both
families. Deflate cannot expand by more than about `1032x` – a structural property of the format –
so the deflate family defaults to `1100x` and never rejects data it could legitimately have
produced. Brotli has no such ceiling: measured on ordinary repetitive input it reaches `9 000x`
for a repeated short string, `21 000x` for a repeated sentence and `80 660x` for a megabyte of
zeros. It therefore has no default ratio limit; callers handling untrusted Brotli input must set
an absolute output limit.

[`DecompressionLimits`][__link16] carries *overrides*, not values: bounds you leave unset keep the
format’s default, so [`DecompressionLimits::default()`][__link17] never silently imposes one format’s
calibration on another.

**A ratio limit is therefore a coarse backstop, not real protection.** For untrusted input, set
[`DecompressionLimits::with_max_output_len`][__link18] to whatever the caller can actually afford to
buffer, and [`DecompressionLimits::with_max_streams`][__link19] when concatenated streams are accepted.
Use [`DecompressionLimits::UNLIMITED`][__link20] only for sources you trust as much as your own process.

Streaming decompression can yield bytes before a final checksum or trailer has been verified.
Treat those bytes as provisional until the operation reports [`Output::Done`][__link21].

## Features

Every format is a separate feature, so a build compiles only the engines it names:

* `gzip` – the `gzip` module and `Format::Gzip`, via `flate2`. The only feature on by
  default, being the encoding most often seen on the wire.
* `deflate` – the `deflate` module and `Format::Deflate`, via `flate2`.
* `zlib` – the `zlib` module and `Format::Zlib`, via `flate2`.
* `brotli` – the `brotli` module and `Format::Brotli`, via the pure-Rust `brotli` crate.
* `zstd` – the `zstd` module and `Format::Zstd`, via `zstd-safe`.
* `futures-stream` – [`CompressionStream`][__link22], presenting compression and decompression as a
  `futures_core::Stream` over any stream of byte sequences.

The deflate-family features share one dependency, so enabling all three costs no more than one.
A build that needs only `brotli` or only `zstd` never compiles `flate2` at all.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/compressors">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbBl8tjF39M8YbgvrtspAvOccboY9vxVOsGMcbO1fWcHMbif5hZIKCaGJ5dGVzYnVmZTAuOS4wgmtjb21wcmVzc29yc2UwLjEuMA
 [__link0]: https://crates.io/crates/bytesbuf/0.9.0
 [__link1]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesView
 [__link10]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressorBuilder::build_format
 [__link11]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources
 [__link12]: https://docs.rs/compressors/0.1.0/compressors/?search=Resources::enable_pooling
 [__link13]: https://docs.rs/compressors/0.1.0/compressors/?search=Format::compress
 [__link14]: https://docs.rs/compressors/0.1.0/compressors/?search=Format::decompress
 [__link15]: https://docs.rs/compressors/0.1.0/compressors/?search=Format::decompress_with_limits
 [__link16]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressionLimits
 [__link17]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressionLimits::default
 [__link18]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressionLimits::with_max_output_len
 [__link19]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressionLimits::with_max_streams
 [__link2]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesBuf
 [__link20]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressionLimits::UNLIMITED
 [__link21]: https://docs.rs/compressors/0.1.0/compressors/?search=Output::Done
 [__link22]: https://docs.rs/compressors/0.1.0/compressors/?search=CompressionStream
 [__link3]: https://docs.rs/compressors/0.1.0/compressors/fn.compress.html
 [__link4]: https://docs.rs/compressors/0.1.0/compressors/fn.decompress.html
 [__link5]: https://docs.rs/compressors/0.1.0/compressors/?search=gzip::Compressor
 [__link6]: https://docs.rs/compressors/0.1.0/compressors/?search=gzip::Decompressor
 [__link7]: https://docs.rs/compressors/0.1.0/compressors/?search=core::Compression
 [__link8]: https://docs.rs/compressors/0.1.0/compressors/?search=core::Compression
 [__link9]: https://docs.rs/compressors/0.1.0/compressors/?search=Format
