// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compresses and decompresses HTTP message bodies.
//!
//! ```
//! # #[cfg(feature = "gzip")] {
//! use compressors::format::Format;
//! use http_compression::Compression;
//! let layer = Compression::client().decompress_responses(&[Format::Gzip]);
//! # let _ = layer;
//! # }
//! ```

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_compression/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_compression/favicon.ico"
)]

//! [`Compression`] transforms HTTP bodies. It compresses small buffered bodies
//! directly. It transforms larger or streaming bodies as they are read.
//! Decompression always returns a streaming body, including for buffered input.
//! The layer uses `Content-Encoding` for decompression and `Accept-Encoding` for
//! response compression. It preserves body policies and trailers. It removes
//! stale `Content-Digest` and `Repr-Digest` values after a transformation.
//!
//! Select a client or server role when you create the layer:
//!
//! | Role | Configured with | Transforms |
//! |---|---|---|
//! | [`Client`] | [`decompress_responses`][CompressionLayer::decompress_responses] | the response coming back |
//! | [`Server`] | [`decompress_requests`][CompressionLayer::decompress_requests] | the request coming in |
//! | [`Server`] | [`compress_responses`][CompressionLayer::compress_responses] | the response going out |
//!
//! Each transformation is off until you configure it. The role limits which
//! transformations are available. Both roles support
//! [`limits`][CompressionLayer::limits],
//! [`resources`][CompressionLayer::resources], and
//! [`on_unsupported`][CompressionLayer::on_unsupported].
//!
//! Clients do not change request bodies. Servers do not decompress response
//! bodies. Servers skip server-sent events, native gRPC, and common compressed
//! media types. A
//! [`content-type allowlist`][CompressionLayer::compressible_types] can restrict
//! response compression further.
//!
//! Response compression uses [`Level::FAST`][compressors::Level::FAST] by
//! default. Use [`level`][CompressionLayer::level] to change it. If a streaming
//! body pauses, the compressor flushes buffered output. The consumer can then
//! decompress each sent part before the body ends.
//!
//! For an eligible successful response with a complete, nonempty body, the
//! server layer can return an empty `406 Not Acceptable` response. It does this
//! if no content coding is acceptable and the caller rejects `identity`. It
//! also does this for an existing unacceptable `Content-Encoding`. `HEAD`,
//! successful `CONNECT`, statuses that cannot carry a body, known-empty bodies,
//! partial responses, and responses with `Content-Range` bypass this
//! negotiation.
//!
//! ```
//! # #[cfg(feature = "gzip")] {
//! use std::num::NonZeroU64;
//!
//! use compressors::DecompressorLimits;
//! use compressors::format::Format;
//! use http_compression::Compression;
//! let client = Compression::client()
//!     .decompress_responses(&[Format::Gzip])
//!     .limits(
//!         DecompressorLimits::new().max_output_len(NonZeroU64::new(16 * 1024 * 1024).unwrap()),
//!     );
//!
//! let server = Compression::server()
//!     .decompress_requests(&[Format::Gzip])
//!     .compress_responses(&[Format::Gzip]);
//! # let _ = (client, server);
//! # }
//! ```
//!
//! A decompressed message stores [`OriginalBody`] in its extensions. This value
//! records the original compression formats. It also records the first
//! `Content-Length` field value if it can parse that value as a `u64`.
//!
//! # Formats
//!
//! A transformation needs at least one format. The `gzip`, `deflate`, `brotli`,
//! and `zstd` features enable their matching formats.
//!
//! HTTP `deflate` uses zlib-wrapped DEFLATE. The `deflate` feature therefore
//! enables [`Format::Zlib`][compressors::format::Format]. Raw RFC 1951 DEFLATE
//! has no HTTP content-coding token, so the crate ignores it.
//!
//! # Bounds
//!
//! Each decompression stage limits output to 64 MiB and 1,024 compressed
//! streams by default. These limits also apply to streaming bodies. Use
//! [`limits`][CompressionLayer::limits] to change them. To remove them, use
//! [`unbounded_output_len`][compressors::DecompressorLimits::unbounded_output_len] and
//! [`unbounded_streams`][compressors::DecompressorLimits::unbounded_streams] methods.
//! An encoded request or response can declare at most 16 enabled compression
//! coding layers. This limit is fixed.
//!
//! # Errors
//!
//! Small buffered compression sets `Content-Length` to the final byte count and
//! removes `Transfer-Encoding`. Decompression and streaming compression remove
//! `Content-Length` because the final size is unknown.
//!
//! A small buffered compression error fails the handler call. Other data errors
//! fail the body read. A failure has one of these labels:
//!
//! | Label | What happened |
//! |-------|---------------|
//! | `compression_invalid` | The body was malformed for its declared compression format |
//! | `compression_limit_exceeded` | The body exceeds a decompression or content-coding limit |
//! | `compression_unsupported` | A recognized but disabled format was seen under [`UnsupportedCompression::Fail`] |
//!
//! An unknown or invalid `Content-Encoding` is not an error under either
//! unsupported-format policy. The crate leaves the body and its headers
//! unchanged.

mod body;
mod compression;
mod error;
mod negotiate;

pub(crate) const CONTENT_DIGEST_HEADER: &str = "content-digest";
pub(crate) const REPR_DIGEST_HEADER: &str = "repr-digest";

pub use compression::{Client, Compression, CompressionLayer, DEFAULT_COMPRESSIBLE_TYPES, OriginalBody, Server, UnsupportedCompression};
pub use error::CompressibleTypeError;
