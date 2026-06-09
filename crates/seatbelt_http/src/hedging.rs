// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-specific hedging middleware.
//!
//! Provides the [`HttpHedging`] and [`HttpHedgingLayer`] type aliases that
//! specialize the [`seatbelt`] hedging middleware for HTTP, plus the
//! [`HttpHedgingLayerExt`] trait for HTTP-aware recovery classification and
//! request cloning.

use seatbelt::hedging::{Hedging, HedgingLayer};
use seatbelt::typestates::Set;

use crate::http_recovery::detect_recovery;
use crate::{HttpClone, HttpRecovery, HttpRequest, HttpResponse};

/// A layer that adds hedging handling to HTTP requests.
///
/// This type alias specializes [`HedgingLayer`] for HTTP operations.
pub type HttpHedgingLayer<S1 = Set, S2 = Set> = HedgingLayer<HttpRequest, http_extensions::Result<HttpResponse>, S1, S2>;

/// A middleware that applies hedging to HTTP requests for tail-latency reduction.
///
/// This type alias specializes [`Hedging`] for HTTP operations.
pub type HttpHedging<S> = Hedging<HttpRequest, http_extensions::Result<HttpResponse>, S>;

/// Extensions adding HTTP-specific configuration for [`HedgingLayer`].
pub trait HttpHedgingLayerExt<S1, S2>: sealed::Sealed {
    /// Configures the hedging layer with sensible defaults for HTTP requests.
    ///
    /// Applies:
    ///
    /// - [`http_clone`][Self::http_clone] with [`HttpClone::safe_only`], so
    ///   only safe methods (e.g. `GET`, `HEAD`) are hedged.
    /// - [`http_recovery`][Self::http_recovery] with the default recovery
    ///   (5xx, request timeouts, and `429 Too Many Requests` are transient).
    ///
    /// Further customize via chained methods such as
    /// [`max_hedged_attempts`][HedgingLayer::max_hedged_attempts] or
    /// [`hedging_delay`][HedgingLayer::hedging_delay].
    fn http_configure_defaults(self) -> HttpHedgingLayer;

    /// Enables cloning of the HTTP request for each hedging attempt, using
    /// the given [`HttpClone`] strategy.
    ///
    /// The request is cloned via
    /// [`HttpRequestExt::try_clone`][http_extensions::HttpRequestExt::try_clone].
    /// If the method is not eligible under the chosen strategy or the body
    /// cannot be cloned, the hedging attempt is skipped.
    ///
    /// The current [`Attempt`][seatbelt::Attempt] is inserted into the
    /// request's extensions. If the extensions contain a
    /// [`Router`][http_extensions::routing::Router], it re-resolves the
    /// request URI on every hedged attempt after the first.
    fn http_clone(self, clone_strategy: HttpClone) -> HttpHedgingLayer<Set, S2>;

    /// Configures recovery classification for transient HTTP failures.
    ///
    /// Responses classified as transient cause the hedging middleware to keep
    /// waiting for other in-flight requests; non-transient responses are
    /// returned immediately. Errors are evaluated via their own
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
    /// # use seatbelt_http::hedging::{HttpHedgingLayer, HttpHedgingLayerExt};
    /// # fn example(layer: HttpHedgingLayer) {
    /// let hedging_layer = layer.http_recovery(HttpRecovery::default());
    /// # }
    /// ```
    ///
    /// Custom recovery function:
    ///
    /// ```rust
    /// # use http::StatusCode;
    /// # use http_extensions::{HttpResponse, ResponseExt, StatusExt};
    /// # use seatbelt_http::HttpRecovery;
    /// # use seatbelt_http::hedging::{HttpHedgingLayer, HttpHedgingLayerExt};
    /// use seatbelt::RecoveryInfo;
    /// # fn example(layer: HttpHedgingLayer) {
    /// let hedging_layer = layer.http_recovery(|response: &HttpResponse| {
    ///     if response.status() == StatusCode::TOO_MANY_REQUESTS {
    ///         // Do not treat 429 as transient.
    ///         return RecoveryInfo::never();
    ///     }
    ///     response.recovery()
    /// });
    /// # }
    /// ```
    fn http_recovery(self, recovery: impl Into<HttpRecovery>) -> HttpHedgingLayer<S1, Set>;
}

impl<S1, S2> HttpHedgingLayerExt<S1, S2> for HttpHedgingLayer<S1, S2> {
    fn http_configure_defaults(self) -> HttpHedgingLayer {
        self.http_clone(HttpClone::default()).http_recovery(HttpRecovery::default())
    }

    fn http_clone(self, clone_strategy: HttpClone) -> HttpHedgingLayer<Set, S2> {
        self.clone_input_with(move |request, args| clone_strategy.try_clone(request, args.attempt(), None))
    }

    fn http_recovery(self, recovery: impl Into<HttpRecovery>) -> HttpHedgingLayer<S1, Set> {
        let recovery = recovery.into();

        self.recovery_with(move |out, args| detect_recovery(out, &recovery, args.clock()))
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<S1, S2> Sealed for HttpHedgingLayer<S1, S2> {}
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

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
    fn hedging_recovers_with_safe_methods() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpHedging::layer("test", &context).http_configure_defaults(),
            FakeHandler::from_status_codes([StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]),
        )
            .into_service();

        let request = HttpRequestBuilder::new_fake().uri("https://example.com").build().unwrap();

        let response = block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn hedging_fails_with_unsafe_methods() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let service = (
            HttpHedging::layer("test", &context).http_configure_defaults(),
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
    fn hedging_routes_hedged_attempts_with_custom_router() {
        // Verify that the request URI seen by the handler on each attempt
        // reflects the routing decision produced by a custom `Router`. The
        // first attempt uses the original target, while hedged attempts must
        // be re-routed through the router before being dispatched.
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = crate::HttpResilienceContext::new(&clock);

        let captured_uris: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_uris_for_handler = Arc::clone(&captured_uris);

        let handler = FakeHandler::from_fn(move |request: HttpRequest| {
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
                    1 => BaseUri::from_static("https://hedge-1.example.com"),
                    _ => BaseUri::from_static("https://hedge-2.example.com"),
                })
            },
            true,
        )
        .conflict_policy(BaseUriConflict::UseRouted);

        let service = (
            HttpHedging::layer("test", &context)
                .http_configure_defaults()
                .max_hedged_attempts(2)
                .hedging_delay(Duration::from_millis(10)),
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
                // Hedged attempt 1: routed to the first alternate endpoint.
                "https://hedge-1.example.com/items".to_string(),
                // Hedged attempt 2: routed to the second alternate endpoint.
                "https://hedge-2.example.com/items".to_string(),
            ],
        );
    }
}
