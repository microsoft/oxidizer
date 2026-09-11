<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Http Compression Logo" width="96">

# Http Compression

[![crate.io](https://img.shields.io/crates/v/http_compression.svg)](https://crates.io/crates/http_compression)
[![docs.rs](https://docs.rs/http_compression/badge.svg)](https://docs.rs/http_compression)
[![MSRV](https://img.shields.io/crates/msrv/http_compression)](https://crates.io/crates/http_compression)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Compresses and decompresses HTTP message bodies.

```rust
use compressors::format::Format;
use http_compression::Compression;
use http_extensions::HttpBodyBuilder;

let layer =
    Compression::client(HttpBodyBuilder::new_fake()).decompress_responses(&[Format::Gzip]);
```

[`compressors`][__link0] transforms a stream of bytes. This crate applies that to
HTTP messages: reading `Content-Encoding`, negotiating `Accept-Encoding`,
and replacing a body with one that compresses or decompresses as it is read.
The trailers and the policies the body already carried travel through intact.

Everything runs through one handler, [`Compression`][__link1], because a client and
a server want different parts of the same job. The role is chosen up front
and decides which toggles exist:

|Role|Configured with|Transforms|
|----|---------------|----------|
|[`Client`][__link2]|[`decompress_responses`][__link3]|the response coming back|
|[`Server`][__link4]|[`decompress_requests`][__link5]|the request coming in|
|[`Server`][__link6]|[`compress_responses`][__link7]|the response going out|

Each is off until it is configured. Because the toggles are scoped to the
role, a misconfiguration is a compile error rather than a subtle one: a
client cannot ask to compress a response it has just received, and a server
cannot advertise `Accept-Encoding` on a request that is arriving rather than
leaving. [`limits`][__link8],
[`resources`][__link9] and
[`on_unsupported`][__link10] mean the same thing
either way, so they are available on both.

Clients leave request bodies unchanged, and servers leave response decompression
to their callers. Servers skip server-sent events, native gRPC and common
already-compressed media types. A
[`content-type allowlist`][__link11] can restrict
response compression further.

Response compression defaults to [`Level::FAST`][__link12],
adjustable with [`level`][__link13]. When a streaming body pauses,
buffered compression output is flushed so the consumer can decompress the bytes
already sent without waiting for the body to end.

```rust
use compressors::DecompressorLimits;
use compressors::format::Format;
use http_compression::Compression;

let client = Compression::client(body_builder.clone())
    .decompress_responses(&[Format::Gzip])
    .limits(DecompressorLimits::new());

let server = Compression::server(body_builder)
    .decompress_requests(&[Format::Gzip])
    .compress_responses(&[Format::Gzip]);
```

Decompressed messages carry [`OriginalBody`][__link14] in their extensions. It records the
original compression formats and `Content-Length` after those headers have been
removed from the decompressed request or response.

## Which formats

Nothing happens until one of the three transformations is given formats, and
only formats compiled in by the `gzip`, `deflate`, `brotli` and `zstd`
features can be named.

HTTP `deflate` is zlib-wrapped DEFLATE, so the `deflate` feature enables
[`Format::Zlib`][__link15]. Raw RFC 1951 DEFLATE has no
HTTP content-coding token and is ignored when supplied.

## Bounds

The streaming decompressor applies no bounds of its own, so whatever is passed to
[`limits`][__link16] is the only thing standing between the
reader and a decompression bomb. Anything that retains a decompressed body should
set [`max_output_len`][__link17].

## Errors

Every transformation is lazy, so a malformed or oversized body fails when it
is read rather than when its headers arrive. Failures carry one of three labels:

|Label|What happened|
|-----|-------------|
|`compression_invalid`|The body was malformed for its declared compression format|
|`compression_limit_exceeded`|Decompression would have exceeded the configured limits|
|`compression_unsupported`|An unavailable format was seen under [`UnsupportedCompression::Fail`][__link18]|

A `Content-Encoding` that cannot be parsed is not an error: the body is left
exactly as it arrived.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_compression">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb8wIS6xG2z5YbweSeKJoDOZgbKp1v1CW-aokbUwyBzgNJ-DBhZIKCa2NvbXByZXNzb3JzZTAuMS4wgnBodHRwX2NvbXByZXNzaW9uZTAuMS4w
 [__link0]: https://crates.io/crates/compressors/0.1.0
 [__link1]: https://docs.rs/http_compression/0.1.0/http_compression/?search=Compression
 [__link10]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::on_unsupported
 [__link11]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::compressible_types
 [__link12]: https://docs.rs/compressors/0.1.0/compressors/?search=Level::FAST
 [__link13]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::level
 [__link14]: https://docs.rs/http_compression/0.1.0/http_compression/?search=OriginalBody
 [__link15]: https://docs.rs/compressors/0.1.0/compressors/?search=format::Format
 [__link16]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::limits
 [__link17]: https://docs.rs/compressors/0.1.0/compressors/?search=DecompressorLimits
 [__link18]: https://docs.rs/http_compression/0.1.0/http_compression/?search=UnsupportedCompression::Fail
 [__link2]: https://docs.rs/http_compression/0.1.0/http_compression/?search=Client
 [__link3]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::decompress_responses
 [__link4]: https://docs.rs/http_compression/0.1.0/http_compression/?search=Server
 [__link5]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::decompress_requests
 [__link6]: https://docs.rs/http_compression/0.1.0/http_compression/?search=Server
 [__link7]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::compress_responses
 [__link8]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::limits
 [__link9]: https://docs.rs/http_compression/0.1.0/http_compression/?search=CompressionLayer::resources
