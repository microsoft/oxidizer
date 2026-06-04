// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for HTTP resilience middleware (retry & circuit breaker).

#![allow(clippy::unwrap_used, reason = "test code")]

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::resilience::breaker::{HttpBreaker, HttpBreakerLayerExt};
use fetch::resilience::retry::{HttpRetry, HttpRetryLayerExt};
use fetch::{HttpClient, HttpResponseBuilder};
use http::{Method, StatusCode};
use http_extensions::HttpError;
use layered::Stack;
use ohno::ErrorExt;
use seatbelt::Recovery;
use seatbelt::retry::Attempt;
use tick::ClockControl;

const ALL_HTTP_METHODS: &[Method] = &[
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::DELETE,
    Method::HEAD,
    Method::OPTIONS,
    Method::CONNECT,
    Method::TRACE,
    Method::PATCH,
];

// ── Retry integration tests ──────────────────────────────────────

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn retry_defaults_for_methods() {
    let client = create_retry_client(StatusCode::INTERNAL_SERVER_ERROR);

    for method in ALL_HTTP_METHODS {
        let response = client.request(method, "https://example.com").fetch().await.unwrap();

        let attempt = response.extensions().get::<Attempt>().copied();

        if method.is_safe() {
            assert_eq!(attempt.unwrap(), Attempt::new(3, true));
        } else {
            assert_eq!(attempt.unwrap(), Attempt::new(0, false));
        }
    }
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn retry_defaults_non_transient_codes() {
    let client = create_retry_client(StatusCode::BAD_REQUEST);
    let response = client.get("https://example.com").fetch().await.unwrap();

    assert_eq!(response.extensions().get::<Attempt>().copied().unwrap(), Attempt::new(0, false));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn retry_defaults_restore_requests() {
    let handler = FakeHandler::from_http_error(|req| {
        let index = req.extensions().get::<Attempt>().copied().unwrap().index();
        HttpError::unavailable(format!("unavailable-{index}")).with_request(req)
    });
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(handler, FakeDeps { clock })
        .custom_pipeline(move |dispatch, context| {
            let layer = HttpRetry::layer("dummy", context.resilience_context())
                .http_configure_defaults()
                .handle_unavailable(true);
            (layer, dispatch).into_service()
        })
        .build();

    // Send non-cloneable body, so we rely on restoring the request from error
    let error = client
        .get("https://example.com")
        .stream(futures::stream::empty::<Result<bytesbuf::BytesView, fetch::HttpError>>())
        .fetch()
        .await
        .unwrap_err();

    assert_eq!(error.message(), "unavailable-3");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn retry_defaults_non_cloneable_body() {
    let client = create_retry_client(StatusCode::INTERNAL_SERVER_ERROR);
    let response = client
        .get("https://example.com")
        .stream(futures::stream::empty::<Result<bytesbuf::BytesView, fetch::HttpError>>())
        .fetch()
        .await
        .unwrap();

    assert_eq!(response.extensions().get::<Attempt>().unwrap().index(), 0);
}

fn create_retry_client(status: StatusCode) -> HttpClient {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    HttpClient::builder_fake(status, FakeDeps { clock })
        .custom_pipeline(move |dispatch, context| {
            let layer = HttpRetry::layer("dummy", context.resilience_context())
                .http_configure_defaults()
                .handle_unavailable(true);
            (layer, dispatch).into_service()
        })
        .build()
}

// ── Breaker integration tests ────────────────────────────────────

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_defaults_trips_on_server_errors() {
    let client = create_breaker_client(StatusCode::INTERNAL_SERVER_ERROR);
    for _ in 0..200 {
        let _ = client.get("https://example.com").fetch().await;
    }

    let error = client.get("https://example.com").fetch().await.unwrap_err();

    assert!(error.message().contains("circuit breaker"));
    assert_eq!(error.recovery().kind(), seatbelt::RecoveryKind::Unavailable);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_defaults_does_not_trip_on_success() {
    let client = create_breaker_client(StatusCode::OK);

    for _ in 0..200 {
        let response = client.get("https://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_defaults_does_not_trip_on_client_errors() {
    let client = create_breaker_client(StatusCode::BAD_REQUEST);

    for _ in 0..200 {
        let response = client.get("https://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_rejected_request_error_attaches_request() {
    let client = create_breaker_client(StatusCode::INTERNAL_SERVER_ERROR);
    for _ in 0..200 {
        let _ = client.get("https://example.com").fetch().await;
    }

    let mut error = client.get("https://example.com").fetch().await.unwrap_err();

    let restored = error.take_request();
    assert!(restored.is_some());
}

fn create_breaker_client(status: StatusCode) -> HttpClient {
    let handler = FakeHandler::from(HttpResponseBuilder::new_fake().status(status).build().unwrap());
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    HttpClient::builder_fake(handler, FakeDeps { clock })
        .custom_pipeline(move |dispatch, context| {
            let layer = HttpBreaker::layer("test", context.resilience_context())
                .http_configure_defaults()
                .min_throughput(100)
                .failure_threshold(0.5);
            (layer, dispatch).into_service()
        })
        .build()
}
