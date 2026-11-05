// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retry clones carry previous recovery into an injected HTTP router.

#![cfg(feature = "retry")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::StatusCode;
use http_extensions::routing::{BaseUriConflict, Router};
use http_extensions::{FakeHandler, HttpRequestBuilder, HttpResponseBuilder};
use layered::{Service, Stack};
use seatbelt::{Attempt, RecoveryInfo};
use seatbelt_http::retry::{HttpRetry, HttpRetryLayerExt};
use seatbelt_http::{HttpClone, HttpResilienceContext};
use templated_uri::BaseUri;
use tick::ClockControl;

#[test]
fn gamma_survivor_retry_router_receives_exact_previous_recovery_and_attempt() {
    let history = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&history);
    let router = Router::custom(
        move |context| {
            captured
                .lock()
                .unwrap()
                .push((context.attempt(), context.previous_recovery().cloned()));
            Some(BaseUri::from_static("https://alternate.invalid"))
        },
        true,
    )
    .conflict_policy(BaseUriConflict::UseRouted);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::clone(&calls);
    let handler = FakeHandler::from_fn(move |request| {
        let attempt = *request.extensions().get::<Attempt>().unwrap();
        requests.lock().unwrap().push((attempt.index(), request.uri().to_string()));
        HttpResponseBuilder::new_fake()
            .status(if attempt.is_last() {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            })
            .build()
    });
    let clock = ClockControl::new().to_clock();
    let context = HttpResilienceContext::new(&clock);
    let service = (
        HttpRetry::layer("routing", &context)
            .http_clone(HttpClone::all())
            .recovery_with(|_, args| {
                if args.attempt().is_last() {
                    RecoveryInfo::never()
                } else {
                    RecoveryInfo::unavailable().delay(Duration::ZERO)
                }
            })
            .handle_unavailable(true)
            .max_retry_attempts(2),
        handler,
    )
        .into_service();
    let request = HttpRequestBuilder::new_fake()
        .uri("https://primary.invalid/items")
        .extension(router)
        .build()
        .unwrap();
    assert_eq!(
        futures::executor::block_on(service.execute(request)).unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        *history.lock().unwrap(),
        [
            (1, Some(RecoveryInfo::unavailable().delay(Duration::ZERO))),
            (2, Some(RecoveryInfo::unavailable().delay(Duration::ZERO))),
        ]
    );
    assert_eq!(
        *calls.lock().unwrap(),
        [
            (0, String::from("https://primary.invalid/items")),
            (1, String::from("https://alternate.invalid/items")),
            (2, String::from("https://alternate.invalid/items")),
        ]
    );
}

#[test]
fn gamma_survivor_retry_does_not_route_when_router_has_no_alternatives() {
    let router = Router::custom(|_| panic!("a router without alternatives must not be called for retries"), false);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let handler = FakeHandler::from_fn(move |request| {
        captured.lock().unwrap().push(request.uri().to_string());
        HttpResponseBuilder::new_fake().status(StatusCode::SERVICE_UNAVAILABLE).build()
    });
    let clock = ClockControl::new().to_clock();
    let context = HttpResilienceContext::new(&clock);
    let service = (
        HttpRetry::layer("single_route", &context)
            .http_clone(HttpClone::all())
            .recovery_with(|_, _| RecoveryInfo::retry().delay(Duration::ZERO))
            .max_retry_attempts(1),
        handler,
    )
        .into_service();
    let request = HttpRequestBuilder::new_fake()
        .uri("https://primary.invalid/items")
        .extension(router)
        .build()
        .unwrap();
    assert_eq!(
        futures::executor::block_on(service.execute(request)).unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        *requests.lock().unwrap(),
        ["https://primary.invalid/items", "https://primary.invalid/items"]
    );
}
