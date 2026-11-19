<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Http Compression Logo" width="96">

# Http Compression

[![crate.io](https://img.shields.io/crates/v/http_compression.svg)](https://crates.io/crates/http_compression)
[![docs.rs](https://docs.rs/http_compression/badge.svg)](https://docs.rs/http_compression)
[![MSRV](https://img.shields.io/crates/msrv/http_compression)](https://crates.io/crates/http_compression)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Compresses and decompresses HTTP message bodies.

```rust
use compressors::format::Format;
use http_compression::Compression;
let layer = Compression::client().decompress_responses(&[Format::Gzip]);
```

[`Compression`][__link0] transforms HTTP bodies. It compresses small buffered bodies
directly. It transforms larger or streaming bodies as they are read.
Decompression always returns a streaming body, including for buffered input.
The layer uses `Content-Encoding` for decompression and `Accept-Encoding` for
response compression. It preserves body policies and trailers. It removes
stale `Content-Digest` and `Repr-Digest` values after a transformation.

Select a client or server role when you create the layer:

|Role|Configured with|Transforms|
|----|---------------|----------|
|[`Client`][__link1]|[`decompress_responses`][__link2]|the response coming back|
|[`Server`][__link3]|[`decompress_requests`][__link4]|the request coming in|
|[`Server`][__link5]|[`compress_responses`][__link6]|the response going out|

Each transformation is off until you configure it. The role limits which
transformations are available. Both roles support
[`limits`][__link7],
[`resources`][__link8], and
[`on_unsupported`][__link9].

Clients do not change request bodies. Servers do not decompress response
bodies. Servers skip server-sent events, native gRPC, and common compressed
media types. A
[`content-type allowlist`][__link10] can restrict
response compression further.

Response compression uses [`Level::FAST`][__link11] by
default. Use [`level`][__link12] to change it. If a streaming
body pauses, the compressor flushes buffered output. The consumer can then
decompress each sent part before the body ends.

For an eligible successful response with a complete body that is not known
to be empty, the server layer can return an empty `406 Not Acceptable`
response. It does this if the caller rejects `identity` and the layer cannot
apply an acceptable coding. A content-type exclusion can prevent that
coding. The layer also returns `406 Not Acceptable` for an existing
unacceptable `Content-Encoding`. `HEAD`, successful `CONNECT`, statuses that
cannot carry a body, known-empty bodies, partial responses, and responses
with `Content-Range` bypass this negotiation.

```rust
use std::num::NonZeroU64;

use compressors::DecompressorLimits;
use compressors::format::Format;
use http_compression::Compression;
let client = Compression::client()
    .decompress_responses(&[Format::Gzip])
    .limits(
        DecompressorLimits::new().max_output_len(NonZeroU64::new(16 * 1024 * 1024).unwrap()),
    );

let server = Compression::server()
    .decompress_requests(&[Format::Gzip])
    .compress_responses(&[Format::Gzip]);
```

A decompressed message stores [`OriginalBody`][__link13] in its extensions. This value
records the original compression formats. It also records the first
`Content-Length` field value if it can parse that value as a `u64`.

## Formats

A transformation needs at least one format. The `gzip`, `deflate`, `brotli`,
and `zstd` features enable their matching formats.

HTTP `deflate` uses zlib-wrapped DEFLATE. The `deflate` feature therefore
enables [`Format::Zlib`][__link14]. Raw RFC 1951 DEFLATE
has no HTTP content-coding token, so the crate ignores it.

## Bounds

Each decompression stage limits output to 64 MiB and 1,024 compressed
streams by default. These limits also apply to streaming bodies. Use
[`limits`][__link15] to change them. To remove them, use
[`unbounded_output_len`][__link16] and
[`unbounded_streams`][__link17] methods.
An encoded request or response can declare at most 16 enabled compression
coding layers. This limit is fixed.

## Errors

Small buffered compression sets `Content-Length` to the final byte count and
removes `Transfer-Encoding`. Decompression and streaming compression remove
`Content-Length` because the final size is unknown.

A small buffered compression error fails the handler call. Other data errors
fail the body read. Errors created by this crate use one of these labels:

|Label|What happened|
|-----|-------------|
|`compression_invalid`|The body was malformed for its declared compression format|
|`compression_limit_exceeded`|The body exceeds a decompression or content-coding limit|
|`compression_unsupported`|A recognized but disabled format was seen under [`UnsupportedCompression::Fail`][__link18]|

A source body error keeps its original label.

An unknown or invalid `Content-Encoding` is not an error under either
unsupported-format policy. The crate leaves the body and its headers
unchanged.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_compression">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbjIF91VUuDpAbNLyEfqDaEpQbkNTJTajiwz4b6v07tnkCpXthZIKCa2NvbXByZXNzb3JzZTAuNS4wgnBodHRwX2NvbXByZXNzaW9uZTAuNS4w
 [__link0]: https://docs.rs/http_compression/0.5.0/http_compression/?search=Compression
 [__link1]: https://docs.rs/http_compression/0.5.0/http_compression/?search=Client
 [__link10]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::compressible_types
 [__link11]: https://docs.rs/compressors/0.5.0/compressors/?search=Level::FAST
 [__link12]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::level
 [__link13]: https://docs.rs/http_compression/0.5.0/http_compression/?search=OriginalBody
 [__link14]: https://docs.rs/compressors/0.5.0/compressors/?search=format::Format
 [__link15]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::limits
 [__link16]: https://docs.rs/compressors/0.5.0/compressors/?search=DecompressorLimits::unbounded_output_len
 [__link17]: https://docs.rs/compressors/0.5.0/compressors/?search=DecompressorLimits::unbounded_streams
 [__link18]: https://docs.rs/http_compression/0.5.0/http_compression/?search=UnsupportedCompression::Fail
 [__link2]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::decompress_responses
 [__link3]: https://docs.rs/http_compression/0.5.0/http_compression/?search=Server
 [__link4]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::decompress_requests
 [__link5]: https://docs.rs/http_compression/0.5.0/http_compression/?search=Server
 [__link6]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::compress_responses
 [__link7]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::limits
 [__link8]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::resources
 [__link9]: https://docs.rs/http_compression/0.5.0/http_compression/?search=CompressionLayer::on_unsupported
