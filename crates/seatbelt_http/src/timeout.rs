// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-specific timeout middleware.
//!
//! Provides the [`HttpTimeout`] and [`HttpTimeoutLayer`] type aliases that
//! specialize the [`seatbelt`] timeout middleware for HTTP, plus the
//! [`HttpTimeoutLayerExt`] trait for HTTP-aware error handling.

use http_extensions::HttpError;
use seatbelt::timeout::{Timeout, TimeoutLayer};
use seatbelt::typestates::Set;

use crate::{HttpRequest, HttpResponse};

/// A layer that adds timeout handling to HTTP requests.
///
/// This type alias specializes [`TimeoutLayer`] for HTTP operations.
pub type HttpTimeoutLayer<S1 = Set, S2 = Set> = TimeoutLayer<HttpRequest, http_extensions::Result<HttpResponse>, S1, S2>;

/// A middleware that applies timeouts to HTTP requests.
///
/// This type alias specializes [`Timeout`] for HTTP operations.
pub type HttpTimeout<S> = Timeout<HttpRequest, http_extensions::Result<HttpResponse>, S>;

/// Extensions adding HTTP-specific configuration for [`TimeoutLayer`].
pub trait HttpTimeoutLayerExt<S1, S2>: sealed::Sealed {
    /// Maps a timeout into an [`HttpError::timeout`].
    ///
    /// The resulting error is classified as retryable, so it can be recovered
    /// by an outer [`HttpRetry`][crate::retry::HttpRetry] layer and counted as
    /// a failure by an outer [`HttpBreaker`][crate::breaker::HttpBreaker]
    /// layer.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use seatbelt_http::timeout::{HttpTimeout, HttpTimeoutLayer, HttpTimeoutLayerExt};
    /// # use seatbelt_http::HttpResilienceContext;
    /// # fn example(context: &HttpResilienceContext) {
    /// let layer = HttpTimeout::layer("my_timeout", context)
    ///     .http_timeout_error()
    ///     .timeout(Duration::from_secs(30));
    /// # }
    /// ```
    fn http_timeout_error(self) -> HttpTimeoutLayer<S1, Set>;
}

impl<S1, S2> HttpTimeoutLayerExt<S1, S2> for HttpTimeoutLayer<S1, S2> {
    fn http_timeout_error(self) -> HttpTimeoutLayer<S1, Set> {
        self.timeout_error(|args| HttpError::timeout(args.timeout()))
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<S1, S2> Sealed for HttpTimeoutLayer<S1, S2> {}
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::executor::block_on;
    use http::StatusCode;
    use http_extensions::{FakeHandler, HttpRequestBuilder};
    use layered::{Service, Stack};
    use seatbelt::{Recovery, RecoveryKind};
    use tick::ClockControl;

    use super::*;
    use crate::HttpResilienceContext;

    #[test]
    fn http_timeout_error_closure_returns_timeout_kind() {
        let timeout = Duration::from_secs(30);
        let error = HttpError::timeout(timeout);

        assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    }

    #[test]
    fn timeout_fires_on_slow_handler() {
        let handler = FakeHandler::never_completes();
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = HttpResilienceContext::new(&clock);

        let service = (
            HttpTimeout::layer("test", &context)
                .http_timeout_error()
                .timeout(Duration::from_secs(5)),
            handler,
        )
            .into_service();

        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();

        let error = block_on(service.execute(request)).unwrap_err();
        assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    }

    #[test]
    fn fast_handler_succeeds_within_timeout() {
        let handler = FakeHandler::from(StatusCode::OK);
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = HttpResilienceContext::new(&clock);

        let service = (
            HttpTimeout::layer("test", &context)
                .http_timeout_error()
                .timeout(Duration::from_secs(30)),
            handler,
        )
            .into_service();

        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();

        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
