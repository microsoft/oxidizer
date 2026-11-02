// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Re-exports the HTTP-specific [`seatbelt_http`] extensions for [`seatbelt`].
//!
//! The extensions specialize generic middleware for
//! [`HttpRequest`][http_extensions::HttpRequest] and
//! [`Result<HttpResponse>`][http_extensions::Result]. HTTP-specific builder methods start with
//! `http_`.
//!
//! # Configuring the standard pipeline
//!
//! [`HttpClientBuilder::standard_pipeline`][crate::HttpClientBuilder::standard_pipeline] provides a
//! [`StandardRequestPipeline`][crate::pipeline::StandardRequestPipeline]. Change its individual
//! layers in the closure:
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
//!         // Retry idempotent methods instead of only safe methods.
//!         pipeline.retry(|retry| retry.http_clone(HttpClone::idempotent()))
//!     })
//!     .build();
//! # }
//! ```
//!
//! ## Switching to hedging
//!
//! The standard pipeline uses sequential retries by default. Select concurrent hedging to reduce
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
//! [`HttpClientBuilder::custom_pipeline`][crate::HttpClientBuilder::custom_pipeline] replaces the
//! pipeline. Stack [`seatbelt_http`] layers above the dispatch handler:
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
//!         // List the outermost layer first.
//!         (retry, timeout, dispatch).into_service()
//!     })
//!     .build();
//! # }
//! ```

pub use seatbelt_http::{HttpClone, HttpRecovery, HttpResilienceContext, breaker, hedging, retry, timeout};
