// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/seatbelt_http/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/seatbelt_http/favicon.ico")]
#![cfg_attr(
    not(all(feature = "retry", feature = "timeout", feature = "breaker", feature = "hedging")),
    expect(
        rustdoc::broken_intra_doc_links,
        reason = "intra-doc links break when not all features are enabled"
    )
)]

//! HTTP-specific extensions for the [`seatbelt`] resilience middleware.
//!
//! Each [`seatbelt`] middleware is generic over its input and output types.
//! This crate specializes them for [`HttpRequest`] /
//! [`Result<HttpResponse>`][http_extensions::Result] and adds HTTP-aware
//! builder methods, all prefixed with `http_`.
//!
//! # Supported middleware
//!
//! Each middleware lives in its own feature-gated module with specialized
//! type aliases and an extension trait:
//!
//! | Module    | Feature   | Purpose |
//! |-----------|-----------|---------|
//! | `retry`   | `retry`   | Recovery classification, request cloning, request restoration from errors. |
//! | `timeout` | `timeout` | Converts timeout events into HTTP-specific errors. |
//! | `hedging` | `hedging` | Recovery classification and request cloning for tail-latency reduction. |
//! | `breaker` | `breaker` | Recovery classification and rejected-request error handling. |
//!
//! # Shared types
//!
//! - [`HttpRecovery`]: classifies HTTP responses as recoverable. By default,
//!   5xx status codes, `429 Too Many Requests`, and request timeouts are
//!   treated as transient.
//! - [`HttpClone`]: selects which HTTP methods are eligible for cloning
//!   during retries and hedging (safe-only, idempotent, or all).
//! - [`HttpResilienceContext`]: the HTTP specialization of
//!   [`ResilienceContext`][seatbelt::ResilienceContext].

use http_extensions::{HttpRequest, HttpResponse};

/// Shared configuration and runtime context for HTTP resilience middleware.
///
/// HTTP specialization of [`ResilienceContext`][seatbelt::ResilienceContext].
pub type HttpResilienceContext = seatbelt::ResilienceContext<HttpRequest, http_extensions::Result<HttpResponse>>;

#[cfg(feature = "timeout")]
pub mod timeout;

#[cfg(feature = "retry")]
pub mod retry;

#[cfg(feature = "hedging")]
pub mod hedging;

#[cfg(feature = "breaker")]
pub mod breaker;

#[cfg(any(feature = "retry", feature = "hedging", feature = "breaker"))]
mod http_recovery;
#[cfg(any(feature = "retry", feature = "hedging", feature = "breaker"))]
pub use http_recovery::HttpRecovery;

#[cfg(any(feature = "retry", feature = "hedging"))]
mod http_clone;
#[cfg(any(feature = "retry", feature = "hedging"))]
pub use http_clone::HttpClone;
