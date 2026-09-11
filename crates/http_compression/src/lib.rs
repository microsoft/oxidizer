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
//! let layer =
//!     Compression::client(HttpBodyBuilder::new_fake()).decompress_responses(&[Format::Gzip]);
//! # let _ = layer;
//! # }
//! ```

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_compression/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_compression/favicon.ico"
)]

//! [`compressors`] transforms a stream of bytes. This crate applies that to
//! HTTP messages: reading `Content-Encoding`, negotiating `Accept-Encoding`,
//! and replacing a body with one that compresses or decompresses as it is read.
//! The trailers and the policies the body already carried travel through intact.
//!
//! Everything runs through one handler, [`Compression`], because a client and
//! a server want different parts of the same job. The role is chosen up front
//! and decides which toggles exist:
//!
//! | Role | Configured with | Transforms |
//! |---|---|---|
//! | [`Client`] | [`decompress_responses`][CompressionLayer::decompress_responses] | the response coming back |
//! | [`Server`] | [`decompress_requests`][CompressionLayer::decompress_requests] | the request coming in |
//! | [`Server`] | [`compress_responses`][CompressionLayer::compress_responses] | the response going out |
//!
//! Each is off until it is configured. Because the toggles are scoped to the
//! role, a misconfiguration is a compile error rather than a subtle one: a
//! client cannot ask to compress a response it has just received, and a server
//! cannot advertise `Accept-Encoding` on a request that is arriving rather than
//! leaving. [`limits`][CompressionLayer::limits],
//! [`resources`][CompressionLayer::resources] and
//! [`on_unsupported`][CompressionLayer::on_unsupported] mean the same thing
//! either way, so they are available on both.
//!
//! Clients leave request bodies unchanged, and servers leave response decompression
//! to their callers. Servers skip server-sent events, native gRPC and common
//! already-compressed media types. A
//! [`content-type allowlist`][CompressionLayer::compressible_types] can restrict
//! response compression further.
//!
//! Response compression defaults to [`Level::FAST`][compressors::Level::FAST],
//! adjustable with [`level`][CompressionLayer::level]. When a streaming body pauses,
//! buffered compression output is flushed so the consumer can decompress the bytes
//! already sent without waiting for the body to end.
//!
//! ```
//! # #[cfg(feature = "gzip")] {
//! use compressors::DecompressorLimits;
//! use compressors::format::Format;
//! use http_compression::Compression;
//! # use http_extensions::HttpBodyBuilder;
//! # let body_builder = HttpBodyBuilder::new_fake();
//!
//! let client = Compression::client(body_builder.clone())
//!     .decompress_responses(&[Format::Gzip])
//!     .limits(DecompressorLimits::new());
//!
//! let server = Compression::server(body_builder)
//!     .decompress_requests(&[Format::Gzip])
//!     .compress_responses(&[Format::Gzip]);
//! # }
//! ```
//!
//! Decompressed messages carry [`OriginalBody`] in their extensions. It records the
//! original compression formats and `Content-Length` after those headers have been
//! removed from the decompressed request or response.
//!
//! # Which formats
//!
//! Nothing happens until one of the three transformations is given formats, and
//! only formats compiled in by the `gzip`, `deflate`, `brotli` and `zstd`
//! features can be named.
//!
//! HTTP `deflate` is zlib-wrapped DEFLATE, so the `deflate` feature enables
//! [`Format::Zlib`][compressors::format::Format]. Raw RFC 1951 DEFLATE has no
//! HTTP content-coding token and is ignored when supplied.
//!
//! # Bounds
//!
//! The streaming decompressor applies no bounds of its own, so whatever is passed to
//! [`limits`][CompressionLayer::limits] is the only thing standing between the
//! reader and a decompression bomb. Anything that retains a decompressed body should
//! set [`max_output_len`][compressors::DecompressorLimits].
//!
//! # Errors
//!
//! Every transformation is lazy, so a malformed or oversized body fails when it
//! is read rather than when its headers arrive. Failures carry one of three labels:
//!
//! | Label | What happened |
//! |-------|---------------|
//! | `compression_invalid` | The body was malformed for its declared compression format |
//! | `compression_limit_exceeded` | Decompression would have exceeded the configured limits |
//! | `compression_unsupported` | An unavailable format was seen under [`UnsupportedCompression::Fail`] |
//!
//! A `Content-Encoding` that cannot be parsed is not an error: the body is left
//! exactly as it arrived.

mod body;
mod compression;
mod error;
mod negotiate;

pub(crate) const CONTENT_DIGEST_HEADER: &str = "content-digest";

pub use compression::{Client, Compression, CompressionLayer, DEFAULT_COMPRESSIBLE_TYPES, OriginalBody, Server, UnsupportedCompression};
