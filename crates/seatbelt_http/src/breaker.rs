// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-specific circuit breaker middleware.
//!
//! Provides the [`HttpBreaker`] and [`HttpBreakerLayer`] type aliases that
//! specialize the [`seatbelt`] circuit breaker for HTTP, plus the
//! [`HttpBreakerLayerExt`] trait for HTTP-aware recovery classification and
//! rejected-request error handling.
//!
//! By default (via [`HttpBreakerLayerExt::http_configure_defaults`]), the
//! breaker ID is derived from the request URI [`Origin`] (scheme, host,
//! and port), so each origin gets its own independent circuit. Default ports
//! (80 for HTTP, 443 for HTTPS) are omitted, so `https://example.com` and
//! `https://example.com:443` share the same circuit. If the origin cannot be
//! extracted (e.g. for a relative URI), a fallback `"default"` ID is used.

use http::Uri;
use http_extensions::HttpError;
use seatbelt::breaker::{Breaker, BreakerId, BreakerLayer};
use seatbelt::typestates::Set;
use templated_uri::Origin;

use crate::http_recovery::detect_recovery;
use crate::{HttpRecovery, HttpRequest, HttpResponse};

/// A layer that adds circuit breaker handling to HTTP requests.
///
/// This type alias specializes [`BreakerLayer`] for HTTP operations.
pub type HttpBreakerLayer<S1 = Set, S2 = Set> = BreakerLayer<HttpRequest, http_extensions::Result<HttpResponse>, S1, S2>;

/// A middleware that applies circuit breaking to HTTP requests.
///
/// This type alias specializes [`Breaker`] for HTTP operations.
pub type HttpBreaker<S> = Breaker<HttpRequest, http_extensions::Result<HttpResponse>, S>;

/// Extensions adding HTTP-specific configuration for [`BreakerLayer`].
pub trait HttpBreakerLayerExt<S1, S2>: sealed::Sealed {
    /// Configures the breaker layer with sensible defaults for HTTP requests.
    ///
    /// Applies:
    ///
    /// - [`http_recovery`][Self::http_recovery] with the default recovery
    ///   (5xx, request timeouts, and `429 Too Many Requests` count as failures).
    /// - [`http_rejected_request_error`][Self::http_rejected_request_error]
    ///   to return [`HttpError::unavailable`] when the circuit is open.
    /// - A breaker ID derived from the request URI [`Origin`], so each
    ///   origin gets its own circuit.
    ///
    /// Further customize via chained methods such as
    /// [`failure_threshold`][BreakerLayer::failure_threshold],
    /// [`break_duration`][BreakerLayer::break_duration], or
    /// [`breaker_id`][BreakerLayer::breaker_id].
    fn http_configure_defaults(self) -> HttpBreakerLayer;

    /// Configures recovery classification for HTTP responses.
    ///
    /// Responses classified as
    /// [`RecoveryInfo::retry`][seatbelt::RecoveryInfo::retry] or
    /// [`RecoveryInfo::unavailable`][seatbelt::RecoveryInfo::unavailable]
    /// count as failures for the breaker's failure-rate tracking; all other
    /// outcomes count as successes. Errors are evaluated via their own
    /// [`recovery`][seatbelt::Recovery::recovery] method.
    ///
    /// See [`HttpRecovery`] for more details.
    ///
    /// # Examples
    ///
    /// Using the default recovery:
    ///
    /// ```rust
    /// # use seatbelt_http::HttpRecovery;
    /// # use seatbelt_http::breaker::{HttpBreakerLayer, HttpBreakerLayerExt};
    /// # fn example(layer: HttpBreakerLayer) {
    /// let breaker_layer = layer.http_recovery(HttpRecovery::default());
    /// # }
    /// ```
    ///
    /// Custom recovery function:
    ///
    /// ```rust
    /// # use http::StatusCode;
    /// # use http_extensions::{HttpResponse, ResponseExt, StatusExt};
    /// # use seatbelt_http::HttpRecovery;
    /// # use seatbelt_http::breaker::{HttpBreakerLayer, HttpBreakerLayerExt};
    /// use seatbelt::RecoveryInfo;
    /// # fn example(layer: HttpBreakerLayer) {
    /// let breaker_layer = layer.http_recovery(|response: &HttpResponse| {
    ///     if response.status() == StatusCode::TOO_MANY_REQUESTS {
    ///         // Do not count 429 responses as failures.
    ///         return RecoveryInfo::never();
    ///     }
    ///     response.recovery()
    /// });
    /// # }
    /// ```
    fn http_recovery(self, recovery: impl Into<HttpRecovery>) -> HttpBreakerLayer<Set, S2>;

    /// Rejects requests with [`HttpError::unavailable`] when the circuit is
    /// open.
    ///
    /// The rejected request is attached to the error, so an outer
    /// [`HttpRetry`][crate::retry::HttpRetry] layer configured with
    /// [`http_restore_request`][crate::retry::HttpRetryLayerExt::http_restore_request]
    /// can recover it and try alternative endpoints.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use seatbelt_http::breaker::{HttpBreakerLayer, HttpBreakerLayerExt};
    /// # fn example(layer: HttpBreakerLayer) {
    /// let breaker_layer = layer.http_rejected_request_error();
    /// # }
    /// ```
    fn http_rejected_request_error(self) -> HttpBreakerLayer<S1, Set>;
}

impl<S1, S2> HttpBreakerLayerExt<S1, S2> for HttpBreakerLayer<S1, S2> {
    fn http_configure_defaults(self) -> HttpBreakerLayer {
        self.http_recovery(HttpRecovery::default())
            .http_rejected_request_error()
            .breaker_id(|req: &HttpRequest| create_breaker_id(req.uri()))
    }

    fn http_recovery(self, recovery: impl Into<HttpRecovery>) -> HttpBreakerLayer<Set, S2> {
        let recovery = recovery.into();

        self.recovery_with(move |out, args| detect_recovery(out, &recovery, args.clock()))
    }

    fn http_rejected_request_error(self) -> HttpBreakerLayer<S1, Set> {
        self.rejected_input_error(|request, _args| HttpError::unavailable("circuit breaker open").with_request(request))
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<S1, S2> Sealed for HttpBreakerLayer<S1, S2> {}
}

/// Derives a [`BreakerId`] from the given [`Uri`]'s [`Origin`] (scheme, host,
/// and port).
///
/// Default ports (80 for HTTP, 443 for HTTPS) are omitted, so
/// `https://example.com:443` and `https://example.com` map to the same ID.
/// This lets the breaker track state per origin so that failures targeting
/// one host do not affect circuits for others.
///
/// If the origin cannot be extracted (e.g. a relative URI), a fallback ID of
/// `"default"` is returned.
fn create_breaker_id(uri: &Uri) -> BreakerId {
    uri.to_string()
        .parse::<Origin>()
        .ok()
        .map_or_else(|| BreakerId::from("default"), |v| BreakerId::from(v.to_string()))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use http::StatusCode;
    use http_extensions::{FakeHandler, HttpRequestBuilder, HttpResponseBuilder};
    use layered::{Service, Stack};
    use ohno::ErrorExt;
    use seatbelt::{Recovery, RecoveryKind};
    use tick::ClockControl;

    use super::*;

    fn breaker_id(uri: &str) -> BreakerId {
        create_breaker_id(&uri.parse::<Uri>().unwrap())
    }

    #[test]
    fn create_breaker_id_extracts_origin() {
        assert_eq!(breaker_id("https://example.com/path?q=1"), BreakerId::from("https://example.com"));
        assert_eq!(breaker_id("http://example.com/path"), BreakerId::from("http://example.com"));
    }

    #[test]
    fn create_breaker_id_handles_ports() {
        assert_eq!(
            breaker_id("https://example.com:8443/api"),
            BreakerId::from("https://example.com:8443")
        );
        assert_eq!(breaker_id("https://example.com:443/api"), BreakerId::from("https://example.com"));
        assert_eq!(breaker_id("http://example.com:80/api"), BreakerId::from("http://example.com"));
    }

    #[test]
    fn create_breaker_id_distinguishes_origins() {
        assert_ne!(breaker_id("https://a.example.com/path"), breaker_id("https://b.example.com/path"));
        assert_eq!(breaker_id("https://example.com/a"), breaker_id("https://example.com/b"));
    }

    #[test]
    fn create_breaker_id_falls_back_to_default() {
        assert_eq!(breaker_id("/relative/path"), BreakerId::from("default"));
    }

    #[test]
    fn server_errors_trip_breaker() {
        let handler =
            FakeHandler::from_sync_handler(|_req| HttpResponseBuilder::new_fake().status(StatusCode::INTERNAL_SERVER_ERROR).build());
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpBreaker::layer("test", &context)
                .http_configure_defaults()
                .min_throughput(10)
                .failure_threshold(0.5),
            handler,
        )
            .into_service();

        for _ in 0..20 {
            let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();
            let _ = block_on(service.execute(request));
        }

        // Circuit is now open — request is rejected with unavailable error
        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();
        let mut error = block_on(service.execute(request)).unwrap_err();
        assert!(error.message().contains("circuit breaker"));
        assert_eq!(error.recovery().kind(), RecoveryKind::Unavailable);
        assert!(error.take_request().is_some());
    }

    #[test]
    fn success_does_not_trip_breaker() {
        let handler = FakeHandler::from_sync_handler(|_req| HttpResponseBuilder::new_fake().status(StatusCode::OK).build());
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpBreaker::layer("test", &context)
                .http_configure_defaults()
                .min_throughput(10)
                .failure_threshold(0.5),
            handler,
        )
            .into_service();

        for _ in 0..20 {
            let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();
            let response = block_on(service.execute(request)).unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Circuit should still be closed
        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();
        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn client_errors_do_not_trip_breaker() {
        let handler = FakeHandler::from_sync_handler(|_req| HttpResponseBuilder::new_fake().status(StatusCode::BAD_REQUEST).build());
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpBreaker::layer("test", &context)
                .http_configure_defaults()
                .min_throughput(10)
                .failure_threshold(0.5),
            handler,
        )
            .into_service();

        for _ in 0..20 {
            let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();
            let response = block_on(service.execute(request)).unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        // Circuit should still be closed
        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();
        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
