// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(windows)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! WinHTTP-based HTTP transport for the [`fetch`] HTTP client.
//!
//! This Windows-only crate adds a `WinHTTP` transport constructor to
//! [`HttpClient`]. Callers supply the clock, memory pool, and telemetry sink
//! required by the transport through [`WinHttpDeps`].
//!
//! WinHTTP-specific TLS and timeout configuration is available through
//! [`WinHttpTlsConfig`] and [`WinHttpOptions`]. Independently built clients do
//! not share connections.
//!
//! ## Platform and transport behavior
//!
//! - Windows 11 build 22000 or later is required.
//! - The transport currently follows automatic Windows proxy policy and exposes
//!   no proxy override or direct-connection fallback. Future requirements may
//!   add explicit control.
//! - Generic TLS configuration and generic transport options that `WinHTTP`
//!   cannot represent, including finite connection limits and connection
//!   idle/lifetime settings, are accepted but ignored.
//! - The request body is fully sent before response reception begins.
//! - Response trailers exposed by `WinHTTP` are preserved for HTTP/2 and HTTP/3.
//!   HTTP/1.1 permits trailer fields, but `WinHTTP` does not expose them.
//!   Request trailers are rejected.
//!
//! Requests are serviced through the operating system's
//! [WinHTTP](https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp)
//! API.
//!
//! [`fetch`]: https://docs.rs/fetch
//! [`HttpClient`]: https://docs.rs/fetch

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/favicon.ico")]

#[expect(
    clippy::allow_attributes,
    reason = "the bindings facade includes request operations used by the request lifecycle"
)]
#[allow(
    dead_code,
    reason = "the bindings facade includes request operations used by the request lifecycle"
)]
mod bindings;
mod body;
mod builder;
#[expect(clippy::allow_attributes, reason = "callback decoding is consumed by the request lifecycle")]
#[allow(dead_code, reason = "callback decoding is consumed by the request lifecycle")]
mod callback;
#[expect(clippy::allow_attributes, reason = "request callback state is consumed by the request lifecycle")]
#[allow(dead_code, reason = "request callback state is consumed by the request lifecycle")]
mod context;
#[expect(clippy::allow_attributes, reason = "request error mappings are used by the request lifecycle")]
#[allow(dead_code, reason = "request error mappings are used by the request lifecycle")]
mod error;
mod error_labels;
#[expect(clippy::allow_attributes, reason = "connect and request handles are used by the request lifecycle")]
#[allow(dead_code, reason = "connect and request handles are used by the request lifecycle")]
mod handle;
#[expect(clippy::allow_attributes, reason = "request option mappings are used by the request lifecycle")]
#[allow(dead_code, reason = "request option mappings are used by the request lifecycle")]
mod options;
#[expect(
    clippy::allow_attributes,
    reason = "request ownership primitives are consumed by the request lifecycle"
)]
#[allow(dead_code, reason = "request ownership primitives are consumed by the request lifecycle")]
mod request;
mod session;
mod telemetry;
mod tls;
mod transport;

pub use builder::{HttpClientWinHttpExt, WinHttpDeps, WinHttpDepsBuilder};
pub use options::{WinHttpOptions, WinHttpOptionsBuilder};
pub use tls::{WinHttpTlsConfig, WinHttpTlsConfigBuilder};
