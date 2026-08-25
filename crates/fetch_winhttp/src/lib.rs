// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! WinHTTP-based HTTP transport for the [`fetch`] HTTP client.
//!
//! This Windows-only crate adds a WinHTTP transport constructor to
//! [`HttpClient`]. Callers supply the clock, memory pool, and telemetry sink
//! required by the transport through [`WinHttpDeps`].
//!
//! WinHTTP-specific TLS configuration is available through
//! [`WinHttpTlsConfig`]. Independently built clients do
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
//!   automatic discovery and proxy auto-configuration scripts. Callers cannot
//!   override that policy, and it may route a request through a proxy or
//!   send it directly to the origin.
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
//! Failures carry a stable error label and retry guidance. The label is
//! contractual; the retry guidance attached to any particular failure is not,
//! and may change as the transport's classification is refined.
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

// The crate exposes no functionality on non-Windows targets. The documentation
// above is written for the supported platform, so the types it links to are
// absent elsewhere; redirect those links to the published documentation, which
// is built for Windows.
#![cfg_attr(
    not(windows),
    doc = "[`WinHttpDeps`]: https://docs.rs/fetch_winhttp",
    doc = "[`WinHttpTlsConfig`]: https://docs.rs/fetch_winhttp"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
// The attribute this feature enables appears in the WinHTTP modules, which are
// configured out on other platforms, and in the placeholder module's tests. The
// feature is therefore declared only where a use of it survives configuration,
// because an unused feature declaration is itself an error.
#![cfg_attr(all(coverage_nightly, any(windows, test)), feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/favicon.ico")]

// Every module below implements the WinHTTP transport and is therefore
// configured out on other platforms. The gate is applied per item rather than
// at the crate root so that the crate still compiles to an instrumented, if
// empty, library elsewhere; see `linux` below.
#[cfg(windows)]
mod bindings;
#[cfg(windows)]
mod body;
#[cfg(windows)]
mod builder;
#[cfg(windows)]
mod callback;
#[cfg(windows)]
mod context;
#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod error_labels;
#[cfg(windows)]
mod handle;
#[cfg(windows)]
mod operation;
#[cfg(windows)]
mod options;
#[cfg(windows)]
mod query;
#[cfg(windows)]
mod request;
#[cfg(windows)]
mod response_headers;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod telemetry;
#[cfg(windows)]
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod testing;
#[cfg(windows)]
mod tls;
#[cfg(windows)]
mod transport;

#[cfg(not(windows))]
mod linux;

#[cfg(windows)]
pub use builder::{HttpClientWinHttpExt, WinHttpDeps, WinHttpDepsBuilder};
#[cfg(windows)]
pub use tls::{WinHttpTlsConfig, WinHttpTlsConfigBuilder};
