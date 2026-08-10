// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(windows)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! WinHTTP-based HTTP transport for the [`fetch`] HTTP client.
//!
//! This Windows-only crate adds a WinHTTP transport constructor to
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
//! - Proxy selection follows automatic Windows proxy policy, including
//!   automatic discovery and proxy auto-configuration scripts; no proxy
//!   override or direct-connection fallback is exposed.
//! - The connection idle timeout is honored, subject to a platform minimum
//!   window.
//! - Generic TLS configuration and the generic transport options WinHTTP
//!   cannot represent, including finite connection limits and bounded
//!   connection lifetimes, are accepted but ignored.
//! - The request body is fully sent before response reception begins.
//! - A request carrying a `Transfer-Encoding` header is rejected before
//!   anything is sent, because this transport performs request framing itself
//!   and cannot honor a caller-supplied transfer coding. Removing the header
//!   does not change how the body is framed on the wire.
//! - A `Content-Length` header must be a single well-formed value, and
//!   repeated values must agree with each other. When the request body reports
//!   its own length, the header must equal it, and a disagreement fails the
//!   request before anything is sent. When the body cannot report a length, the
//!   header declares it and is taken on trust. A header that survives is sent
//!   in normalized decimal form.
//! - Redirects are not followed, no cookie store is kept, and authentication
//!   challenges are not answered automatically. A redirect response is
//!   returned to the caller as an ordinary response, and `Set-Cookie`,
//!   `Cookie`, and challenge headers pass through as plain data for the caller
//!   to act on. None of these can be re-enabled.
//! - Gzip and deflate response bodies are decoded transparently, and the
//!   `Content-Encoding` and `Content-Length` headers describing the encoded
//!   form are removed. There is no opt-out. Brotli and zstd responses are
//!   delivered still encoded, with their headers intact.
//! - Response trailers exposed by WinHTTP are preserved for HTTP/2 and HTTP/3.
//!   HTTP/1.1 permits trailer fields, but WinHTTP does not expose them.
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

mod bindings;
mod body;
mod builder;
mod callback;
mod context;
mod convert;
mod error;
mod error_labels;
mod handle;
mod operation;
mod options;
mod query;
mod request;
mod response_headers;
mod session;
mod telemetry;
#[cfg(test)]
mod testing;
mod tls;
mod transport;

pub use builder::{HttpClientWinHttpExt, WinHttpDeps, WinHttpDepsBuilder};
pub use options::{WinHttpOptions, WinHttpOptionsBuilder};
pub use tls::{WinHttpTlsConfig, WinHttpTlsConfigBuilder};
