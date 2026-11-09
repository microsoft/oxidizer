// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compresses and decompresses HTTP message bodies.
//!
//! ```
//! # #[cfg(feature = "gzip")] {
//! use compressors::format::Format;
//! use http_compression::Compression;
//! use http_extensions::HttpBodyBuilder;
//!
//! # fn configure(body_builder: HttpBodyBuilder) {
//! let layer = Compression::client(body_builder).decompress_responses(&[Format::Gzip]);
//! # let _ = layer;
//! # }
//! # }
//! ```

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_compression/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_compression/favicon.ico"
)]

//! [`Compression`] transforms HTTP bodies as they are read. It uses
//! `Content-Encoding` to decompress bodies and `Accept-Encoding` to select
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
//! ```
//! # #[cfg(feature = "gzip")] {
//! use std::num::NonZeroU64;
//!
//! use compressors::DecompressorLimits;
//! use compressors::format::Format;
//! use http_compression::Compression;
//! use http_extensions::HttpBodyBuilder;
//!
//! # fn configure(body_builder: HttpBodyBuilder) {
//! let client = Compression::client(body_builder.clone())
//!     .decompress_responses(&[Format::Gzip])
//!     .limits(
//!         DecompressorLimits::new().max_output_len(NonZeroU64::new(16 * 1024 * 1024).unwrap()),
//!     );
//!
//! let server = Compression::server(body_builder)
//!     .decompress_requests(&[Format::Gzip])
//!     .compress_responses(&[Format::Gzip]);
//! # let _ = (client, server);
//! # }
//! # }
//! ```
//!
//! A decompressed message stores [`OriginalBody`] in its extensions. This value
//! records the original compression formats and `Content-Length`.
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
//! A decompressed message can contain at most 16 enabled compression coding
//! layers. This limit is fixed.
//!
//! # Errors
//!
//! Transformations are lazy. A malformed or oversized body fails when it is
//! read, not when its headers arrive. A failure has one of these labels:
//!
//! | Label | What happened |
//! |-------|---------------|
//! | `compression_invalid` | The body was malformed for its declared compression format |
//! | `compression_limit_exceeded` | The body exceeds a decompression or content-coding limit |
//! | `compression_unsupported` | An unavailable format was seen under [`UnsupportedCompression::Fail`] |
//!
//! An invalid `Content-Encoding` is not an error. The crate leaves the body and
//! its headers unchanged.

mod body;
mod compression;
mod error;
mod negotiate;

pub(crate) const CONTENT_DIGEST_HEADER: &str = "content-digest";
pub(crate) const REPR_DIGEST_HEADER: &str = "repr-digest";

pub use compression::{Client, Compression, CompressionLayer, DEFAULT_COMPRESSIBLE_TYPES, OriginalBody, Server, UnsupportedCompression};
pub use error::CompressibleTypeError;
