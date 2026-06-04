// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Re-exports the [`seatbelt_http`] crate which provides HTTP-specific
//! extensions for [`seatbelt`] resilience middleware.
//!
//! These extensions simplify the configuration of HTTP-related resilience
//! functionality by specializing the generic [`seatbelt`] middleware types
//! for [`HttpRequest`][http_extensions::HttpRequest] /
//! [`Result<HttpResponse>`][http_extensions::Result] and exposing HTTP-aware
//! builder methods (all prefixed with `http_`).
//!
//! # Configuring the standard pipeline
//!
//! The easiest way to add resilience is through
//! [`HttpClientBuilder::standard_pipeline`][crate::HttpClientBuilder::standard_pipeline],
//! which gives you a pre-built [`StandardRequestPipeline`][crate::pipeline::StandardRequestPipeline]
//! whose individual layers you can tweak:
//!
//! ```rust
//! # use std::time::Duration;
//! # use fetch::HttpClientBuilder;
//! # use fetch::resilience::HttpClone;
//! # use fetch::resilience::retry::HttpRetryLayerExt;
//! # use http::StatusCode;
//! # use seatbelt::retry::Backoff;
//! # fn example(builder: HttpClientBuilder) {
//! let client = builder
//!     .standard_pipeline(|pipeline, _context| {
//!         // Allow retrying idempotent methods (GET, PUT, DELETE, …)
//!         // instead of only safe methods (GET, HEAD).
//!         pipeline.retry(|retry| retry.http_clone(HttpClone::idempotent()))
//!     })
//!     .build();
//! # }
//! ```
//!
//! ## Switching to hedging
//!
//! The standard pipeline supports two recovery strategies. By default it uses
//! sequential retries, but you can switch to concurrent hedging for lower
//! tail latency:
//!
//! ```rust
//! # use fetch::HttpClientBuilder;
//! # use fetch::pipeline::RecoveryMode;
//! # fn example(builder: HttpClientBuilder) {
//! let client = builder
//!     .standard_pipeline(|pipeline, _context| pipeline.recovery_mode(RecoveryMode::Hedging))
//!     .build();
//! # }
//! ```
//!
//! # Configuring a custom pipeline
//!
//! For full control you can replace the entire pipeline via
//! [`HttpClientBuilder::custom_pipeline`][crate::HttpClientBuilder::custom_pipeline].
//! Build any combination of [`seatbelt_http`] layers and stack them on top of
//! the dispatch handler:
//!
//! ```rust
//! # use std::time::Duration;
//! # use fetch::handlers::Logging;
//! # use fetch::HttpClientBuilder;
//! # use fetch::resilience::HttpRecovery;
//! # use fetch::resilience::retry::{HttpRetry, HttpRetryLayerExt};
//! # use fetch::resilience::timeout::{HttpTimeout, HttpTimeoutLayerExt};
//! # use layered::Stack;
//! # use seatbelt::RecoveryInfo;
//! # fn example(builder: HttpClientBuilder) {
//! let client = builder
//!     .custom_pipeline(|dispatch, ctx| {
//!         let retry = HttpRetry::layer("my_retry", ctx.resilience_context())
//!             .http_configure_defaults()
//!             .max_retry_attempts(2);
//!
//!         let timeout = HttpTimeout::layer("my_timeout", ctx.resilience_context())
//!             .http_timeout_error()
//!             .timeout(Duration::from_secs(5));
//!
//!         // Outermost layer is listed first.
//!         (retry, timeout, dispatch).into_service()
//!     })
//!     .build();
//! # }
//! ```

pub use seatbelt_http::{HttpClone, HttpRecovery, HttpResilienceContext, breaker, hedging, retry, timeout};
