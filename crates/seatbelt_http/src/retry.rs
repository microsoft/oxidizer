// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-specific retry middleware.
//!
//! Provides the [`HttpRetry`] and [`HttpRetryLayer`] type aliases that
//! specialize the [`seatbelt`] retry middleware for HTTP, plus the
//! [`HttpRetryLayerExt`] trait for HTTP-aware recovery classification,
//! request cloning, and error handling.

use http_extensions::HttpError;
use seatbelt::Recovery;
use seatbelt::retry::{Retry, RetryLayer};
use seatbelt::typestates::Set;

use crate::http_recovery::detect_recovery;
use crate::{HttpClone, HttpRecovery, HttpRequest, HttpResponse};

/// A layer that adds retry handling to HTTP requests.
///
/// This type alias specializes [`RetryLayer`] for HTTP operations.
pub type HttpRetryLayer<S1 = Set, S2 = Set> = RetryLayer<HttpRequest, http_extensions::Result<HttpResponse>, S1, S2>;

/// A middleware that applies retries to HTTP requests.
///
/// This type alias specializes [`Retry`] for HTTP operations.
pub type HttpRetry<S> = Retry<HttpRequest, http_extensions::Result<HttpResponse>, S>;

/// Extensions adding HTTP-specific configuration for [`RetryLayer`].
pub trait HttpRetryLayerExt<S1, S2>: sealed::Sealed {
    /// Configures the retry layer with sensible defaults for HTTP requests.
    ///
    /// Applies:
    ///
    /// - [`http_clone`][Self::http_clone] with [`HttpClone::safe_only`], so
    ///   only safe methods (e.g. `GET`, `HEAD`) are retried.
    /// - [`http_recovery`][Self::http_recovery] with the default recovery
    ///   (5xx, request timeouts, and `429 Too Many Requests` are transient).
    /// - [`http_restore_request`][Self::http_restore_request] to recover the
    ///   request from an [`HttpError`] when possible.
    ///
    /// Further customize via chained configuration methods.
    fn http_configure_defaults(self) -> HttpRetryLayer;

    /// Enables cloning of the HTTP request for each retry attempt, using the
    /// given [`HttpClone`] strategy.
    ///
    /// The request is cloned via
    /// [`HttpRequestExt::try_clone`][http_extensions::HttpRequestExt::try_clone].
    /// If the method is not eligible under the chosen strategy or the body
    /// cannot be cloned, no retry is attempted (unless the request can be
    /// recovered via [`http_restore_request`][Self::http_restore_request]).
    ///
    /// The current [`Attempt`][seatbelt::Attempt] is inserted into the
    /// request's extensions. If the extensions contain a
    /// [`Router`][http_extensions::routing::Router], it re-resolves the
    /// request URI on every retry attempt after the first.
    fn http_clone(self, clone_strategy: HttpClone) -> HttpRetryLayer<Set, S2>;

    /// Configures recovery classification for transient HTTP failures.
    ///
    /// Transient responses trigger a retry; when a response includes a
    /// `Retry-After` header, the retry is delayed accordingly. Errors are
    /// evaluated via their own [`recovery`][seatbelt::Recovery::recovery]
    /// method.
    ///
    /// See [`HttpRecovery`] for more details.
    ///
    /// # Examples
    ///
    /// Using the default recovery:
    ///
    /// ```rust
    /// # use seatbelt_http::HttpRecovery;
    /// # use seatbelt_http::retry::{HttpRetryLayer, HttpRetryLayerExt};
    /// # use seatbelt::retry::RetryLayer;
    /// # fn example(layer: HttpRetryLayer) {
    /// let retry_layer = layer.http_recovery(HttpRecovery::default());
    /// # }
    /// ```
    ///
    /// Custom recovery function:
    ///
    /// ```rust
    /// # use http::StatusCode;
    /// # use http_extensions::{HttpResponse, ResponseExt, StatusExt};
    /// # use seatbelt_http::HttpRecovery;
    /// # use seatbelt_http::retry::{HttpRetryLayer, HttpRetryLayerExt};
    /// use seatbelt::RecoveryInfo;
    /// # use seatbelt::retry::RetryLayer;
    /// # fn example(layer: HttpRetryLayer) {
    /// let retry_layer = layer.http_recovery(|response: &HttpResponse| {
    ///     if response.status() == StatusCode::TOO_MANY_REQUESTS {
    ///         // Do not retry 429 responses.
    ///         return RecoveryInfo::never();
    ///     }
    ///     response.recovery()
    /// });
    /// # }
    /// ```
    fn http_recovery(self, recovery: impl Into<HttpRecovery>) -> HttpRetryLayer<S1, Set>;

    /// Restores the original HTTP request from an [`HttpError`] when possible.
    ///
    /// A request can be restored when the error's recovery kind is
    /// [`Unavailable`][seatbelt::RecoveryKind::Unavailable] and the request
    /// has not already been consumed. Requests are attached via
    /// [`HttpError::with_request`]: for example, by the breaker's
    /// [`http_rejected_request_error`][crate::breaker::HttpBreakerLayerExt::http_rejected_request_error].
    fn http_restore_request(self) -> HttpRetryLayer<S1, S2>;
}

impl<S1, S2> HttpRetryLayerExt<S1, S2> for HttpRetryLayer<S1, S2> {
    fn http_configure_defaults(self) -> HttpRetryLayer {
        self.http_clone(HttpClone::default())
            .http_recovery(HttpRecovery::default())
            .http_restore_request()
    }

    fn http_clone(self, clone_strategy: HttpClone) -> HttpRetryLayer<Set, S2> {
        self.clone_input_with(move |request, args| clone_strategy.try_clone(request, args.attempt(), args.previous_recovery()))
    }

    fn http_recovery(self, recovery: impl Into<HttpRecovery>) -> HttpRetryLayer<S1, Set> {
        let recovery = recovery.into();

        self.recovery_with(move |out, args| detect_recovery(out, &recovery, args.clock()))
    }

    fn http_restore_request(self) -> Self {
        self.restore_input_from_error(|error, _args| extract_http_request(error))
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<S1, S2> Sealed for HttpRetryLayer<S1, S2> {}
}

fn extract_http_request(error: &mut HttpError) -> Option<HttpRequest> {
    // We can only restore the request when the error reports an outage.
    if error.recovery().kind() != seatbelt::RecoveryKind::Unavailable {
        return None;
    }

    error.take_request()
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use futures::executor::block_on;
    use http::{Method, StatusCode};
    use http_extensions::routing::{BaseUriConflict, Router};
    use http_extensions::{FakeHandler, HttpRequestBuilder, HttpResponseBuilder};
    use layered::{Service, Stack};
    use seatbelt::Attempt;
    use templated_uri::BaseUri;
    use tick::ClockControl;

    use super::*;

    #[test]
    fn retry_recovers_with_safe_methods() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpRetry::layer("test", &context).http_configure_defaults(),
            FakeHandler::from_status_codes([StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]),
        )
            .into_service();

        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();

        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn retry_fails_with_unsafe_methods() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpRetry::layer("test", &context).http_configure_defaults(),
            FakeHandler::from_status_codes([StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]),
        )
            .into_service();

        let request = HttpRequestBuilder::new_fake()
            .uri("https://example.com")
            .method(Method::POST)
            .build()
            .unwrap();

        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn restore_request_from_unavailable_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&call_count);
        let handler = FakeHandler::from_sync_handler(move |req| {
            let n = counter.fetch_add(1, Ordering::Relaxed);
            if n < 2 {
                Err(HttpError::unavailable("service down").with_request(req))
            } else {
                HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
            }
        });
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpRetry::layer("test", &context)
                .http_configure_defaults()
                .handle_unavailable(true)
                .max_retry_attempts(2),
            handler,
        )
            .into_service();

        // POST cannot be cloned with safe_only — request is restored from error instead
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .build()
            .unwrap();

        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn retry_routes_attempts_with_custom_router() {
        // Verify that the request URI seen by the handler on each retry attempt
        // reflects the routing decision produced by a custom `Router`. The
        // first attempt uses the original target, while subsequent retry
        // attempts must be re-routed through the router before being dispatched.
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let captured_uris: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_uris_for_handler = Arc::clone(&captured_uris);

        let handler = FakeHandler::from_sync_handler(move |request: HttpRequest| {
            captured_uris_for_handler
                .lock()
                .expect("mutex is only accessed in single-threaded test")
                .push(request.uri().to_string());

            let attempt = request.extensions().get::<Attempt>().unwrap();
            let status = if attempt.is_last() {
                StatusCode::OK
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };

            HttpResponseBuilder::new_fake().status(status).build()
        });

        // Custom router that picks a different base URI per attempt index.
        // The first attempt is not re-routed by `HttpClone::try_clone`, so its
        // URI stays the original "primary" target.
        let router = Router::custom(
            |ctx| {
                Some(match ctx.attempt() {
                    1 => BaseUri::from_static("https://retry-1.example.com"),
                    _ => BaseUri::from_static("https://retry-2.example.com"),
                })
            },
            true,
        )
        .conflict_policy(BaseUriConflict::UseRouted);

        let service = (
            HttpRetry::layer("test", &context).http_configure_defaults().max_retry_attempts(2),
            handler,
        )
            .into_service();

        let request = HttpRequestBuilder::new_fake()
            .uri("https://primary.example.com/items")
            .extension(router)
            .build()
            .unwrap();

        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let uris = captured_uris
            .lock()
            .expect("mutex is only accessed in single-threaded test")
            .clone();

        assert_eq!(
            uris,
            vec![
                // First attempt uses the original URI (not re-routed).
                "https://primary.example.com/items".to_string(),
                // Retry attempt 1: routed to the first alternate endpoint.
                "https://retry-1.example.com/items".to_string(),
                // Retry attempt 2: routed to the second alternate endpoint.
                "https://retry-2.example.com/items".to_string(),
            ],
        );
    }
}
