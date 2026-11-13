// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retry and hedging layers honor the configured request-cloning strategy.

#![cfg(any(feature = "retry", feature = "hedging"))]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{Method, StatusCode};
use http_extensions::{FakeHandler, HttpRequest, HttpRequestBuilder, HttpResponseBuilder};
use layered::{Service, Stack};
use seatbelt::{Attempt, RecoveryInfo};
use seatbelt_http::{HttpClone, HttpResilienceContext};
use tick::ClockControl;

#[cfg(feature = "retry")]
#[test]
fn retry_honors_non_default_clone_strategies() {
    use seatbelt_http::retry::{HttpRetry, HttpRetryLayerExt};

    for (strategy, method) in [(HttpClone::all(), Method::POST), (HttpClone::idempotent(), Method::PUT)] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let handler = FakeHandler::from_fn(move |request: HttpRequest| {
            let attempt = request.extensions().get::<Attempt>().unwrap().index();
            captured.lock().unwrap().push((attempt, request.method().clone()));
            HttpResponseBuilder::new_fake()
                .status(if attempt == 0 {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                })
                .build()
        });
        let clock = ClockControl::new().to_clock();
        let context = HttpResilienceContext::new(&clock);
        let service = (
            HttpRetry::layer("clone_strategy", &context)
                .http_clone(strategy)
                .recovery_with(|_, _| RecoveryInfo::retry().delay(Duration::ZERO))
                .max_retry_attempts(1),
            handler,
        )
            .into_service();
        let request = HttpRequestBuilder::new_fake()
            .method(method.clone())
            .uri("https://example.invalid/items")
            .build()
            .unwrap();

        let response = futures::executor::block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*requests.lock().unwrap(), [(0, method.clone()), (1, method)]);
    }
}

#[cfg(feature = "hedging")]
#[test]
fn hedging_honors_non_default_clone_strategies() {
    use seatbelt_http::hedging::{HttpHedging, HttpHedgingLayerExt};

    for (strategy, method) in [(HttpClone::all(), Method::POST), (HttpClone::idempotent(), Method::PUT)] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let handler = FakeHandler::from_fn(move |request: HttpRequest| {
            let attempt = request.extensions().get::<Attempt>().unwrap().index();
            captured.lock().unwrap().push((attempt, request.method().clone()));
            HttpResponseBuilder::new_fake()
                .status(if attempt == 0 {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                })
                .build()
        });
        let clock = ClockControl::new().to_clock();
        let context = HttpResilienceContext::new(&clock);
        let service = (
            HttpHedging::layer("clone_strategy", &context)
                .http_clone(strategy)
                .recovery_with(|_, _| RecoveryInfo::retry().delay(Duration::ZERO))
                .max_hedged_attempts(1)
                .hedging_delay(Duration::ZERO),
            handler,
        )
            .into_service();
        let request = HttpRequestBuilder::new_fake()
            .method(method.clone())
            .uri("https://example.invalid/items")
            .build()
            .unwrap();

        let response = futures::executor::block_on(service.execute(request)).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*requests.lock().unwrap(), [(0, method.clone()), (1, method)]);
    }
}
