// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
//! ## Platform requirements
//!
//! Windows 11 (build 22000) or later, or Windows Server 2025 (build 26100) or
//! later. Windows Server 2022 and earlier are not supported.
//!
//! ## Behavior and limitations
//!
//! WinHTTP owns connection management and much of the HTTP protocol, so some
//! generic `fetch` configuration cannot be represented and some behavior is
//! fixed by the operating system.
//!
//! - Generic TLS configuration, finite connection limits, and bounded
//!   connection lifetimes are accepted and ignored rather than rejected, so a
//!   client that sets them still builds.
//! - Proxy selection follows automatic Windows proxy policy, including
//!   automatic discovery and proxy auto-configuration scripts. There is no
//!   proxy override and no direct-connection fallback.
//! - Redirects are not followed, no cookie store is kept, and authentication
//!   challenges are not answered. Those responses are returned to the caller to
//!   act on, and none of this can be re-enabled.
//! - Request framing is derived from the body, so a caller-supplied
//!   `Transfer-Encoding` is rejected before anything is sent. Code that
//!   forwards an inbound request's headers verbatim is the common case that
//!   trips on this.
//! - Gzip and deflate responses are decoded transparently, with the headers
//!   describing the encoded form removed and no opt-out. Brotli and zstd are
//!   not decoded and arrive still encoded.
//! - A request body that yields trailers fails the request, and does so after
//!   the headers and preceding body data have already been sent. The request
//!   body is sent in full before response reception begins.
//!
//! The full contract - error classification, timeout semantics, and the
//! fidelity of every generic option - is documented in
//! [`docs/design.md`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch_winhttp/docs/design.md).
//!
//! Requests are serviced through the operating system's
//! [WinHTTP](https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp)
//! API.
//!
//! [`fetch`]: https://docs.rs/fetch
//! [`HttpClient`]: https://docs.rs/fetch

// The crate is empty on non-Windows targets. The documentation above is
// deliberately declared before this attribute so that it survives the
// configuration stripping and the crate stays documented on every platform.
// The types it links to do not exist there, so the links are redirected to the
// published documentation, which is built for Windows.
#![cfg_attr(
    not(windows),
    doc = "[`WinHttpDeps`]: https://docs.rs/fetch_winhttp",
    doc = "[`WinHttpTlsConfig`]: https://docs.rs/fetch_winhttp",
    doc = "[`WinHttpOptions`]: https://docs.rs/fetch_winhttp"
)]
#![cfg(windows)]
#![cfg_attr(docsrs, feature(doc_cfg))]
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
