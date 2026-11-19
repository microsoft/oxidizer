// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! WinHTTP transport for the [`fetch`] HTTP client.
//!
//! This Windows-only crate adds a WinHTTP constructor to [`HttpClient`].
//! [`WinHttpDeps`] supplies the clock, memory provider, and telemetry sink.
//! The memory provider uses `bytesbuf::mem::OpaqueMemory`, so callers can select
//! an allocator.
//!
//! [`WinHttpTlsConfig`] provides WinHTTP-specific TLS configuration.
//! Independently built clients do not share connections.
//!
//! ## Platform requirements
//!
//! Windows 11 (build 22000) or later, or Windows Server 2025 (build 26100) or
//! later. Windows Server 2022 and earlier are not supported.
//!
//! ## Behavior and limitations
//!
//! WinHTTP manages connections and much of the HTTP protocol. Some generic
//! `fetch` configuration does not apply, and Windows fixes some behavior.
//!
//! - Generic TLS configuration, finite connection limits, and bounded
//!   connection lifetimes are accepted but ignored. A client that sets them
//!   still builds.
//! - Requests always connect directly to the origin. Proxies are not supported,
//!   Windows proxy configuration is not consulted, and there is no setting that
//!   changes this.
//! - Redirects are not followed. The transport does not keep cookies or answer
//!   authentication challenges. It returns these responses to the caller.
//! - The body determines request framing. A caller-supplied
//!   `Transfer-Encoding` is rejected before the transport sends data.
//! - Gzip and deflate responses are decoded automatically. The transport
//!   removes headers that describe the encoded form. Brotli and zstd responses
//!   remain encoded.
//! - Request trailers fail the request after the transport sends the headers
//!   and earlier body data. The transport sends the full request body before it
//!   receives the response.
//!
//! Failures include a stable error label and retry guidance. The label is part
//! of the contract. The retry guidance can change.
//!
//! The full contract covers error classification, timeout behavior, and each
//! generic configuration value. It is documented in
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
// Only the coverage-anchor tests use this feature. Declare it only when those
// tests compile because an unused feature declaration is an error.
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_winhttp/favicon.ico")]

// `fetch_winhttp_impl` contains the transport. These coverage-anchor modules
// keep one tested item in each platform build. This prevents an empty coverage
// result from appearing as a failed measurement.
#[cfg(not(windows))]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use fetch_winhttp_impl::{HttpClientWinHttpExt, WinHttpDeps, WinHttpDepsBuilder, WinHttpTlsConfig, WinHttpTlsConfigBuilder};
