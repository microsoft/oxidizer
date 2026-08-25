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
//! [`WinHttpDeps`]: https://docs.rs/fetch_winhttp/latest/fetch_winhttp/struct.WinHttpDeps.html
//! [`WinHttpTlsConfig`]: https://docs.rs/fetch_winhttp/latest/fetch_winhttp/struct.WinHttpTlsConfig.html

#![cfg_attr(docsrs, feature(doc_cfg))]
// The attribute this feature enables appears only in the placeholder modules'
// tests, so the feature is declared only where a use of it survives
// configuration; an unused feature declaration is itself an error.
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/favicon.ico")]

// The transport is implemented in `fetch_winhttp_impl`, so this library carries
// no code of its own on any target. The two placeholder modules below keep one
// trivially exercised item in the build per platform, which is what stops the
// coverage tooling from reading the resulting empty measurement as a failed one.
#[cfg(not(windows))]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use fetch_winhttp_impl::{HttpClientWinHttpExt, WinHttpDeps, WinHttpDepsBuilder, WinHttpTlsConfig, WinHttpTlsConfigBuilder};
