// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests verifying that `RequestInfo` (the templated URIs and the
//! resilience [`Attempt`]) flows end-to-end through the real client pipeline
//! onto both the [`HttpResponse`][fetch::HttpResponse] and the
//! [`HttpError`][fetch::HttpError].
//!
//! Unit tests in `handlers::dispatch` cover the forwarding in isolation; these
//! tests exercise the whole client (request building, routing, retry, dispatch)
//! so a regression anywhere along the chain is caught.

#![allow(clippy::unwrap_used, reason = "test code")]

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::resilience::retry::{HttpRetry, HttpRetryLayerExt};
use fetch::{HttpClient, HttpError, ResponseExt};
use http::StatusCode;
use http_extensions::routing::{BaseUriConflict, Router};
use layered::Stack;
use seatbelt::retry::Attempt;
use std::time::Duration;
use templated_uri::BaseUri;
use tick::ClockControl;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn request_info_flows_to_successful_response() {
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).build();

    let response = client.get("https://example.com/v1/items").fetch().await.unwrap();

    let info = response.request_info().expect("RequestInfo should be forwarded onto the response");

    // The caller-supplied templated target is preserved untouched.
    assert_eq!(
        info.original_uri.as_ref().unwrap().to_string().declassify_ref(),
        "https://example.com/v1/items"
    );
    // Routing records the resolved URI, and the retry layer records the
    // attempt: the first (index 0) attempt of a multi-attempt operation, so it
    // is not flagged as the last one.
    assert!(info.routed_uri.is_some(), "routed_uri should be recorded by the router");
    assert_eq!(response.attempt(), Some(Attempt::new(0, false)));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn request_info_attempt_reflects_retries_on_response() {
    // Auto-advancing clock so the retry back-off delays resolve instantly.
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(StatusCode::INTERNAL_SERVER_ERROR, FakeDeps { clock })
        .custom_pipeline(|dispatch, context| {
            let layer = HttpRetry::layer("test", context.resilience_context()).http_configure_defaults();
            (layer, dispatch).into_service()
        })
        .build();

    let response = client.get("https://example.com/v1/items").fetch().await.unwrap();

    let info = response.request_info().expect("RequestInfo should be forwarded onto the response");

    // `original_uri` survives untouched across every retry attempt.
    assert_eq!(
        info.original_uri.as_ref().unwrap().to_string().declassify_ref(),
        "https://example.com/v1/items"
    );
    // A safe method is retried up to the default 3 times, so the final attempt
    // forwarded onto the response is index 3 (the last one).
    assert_eq!(response.attempt(), Some(Attempt::new(3, true)));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn request_info_flows_to_error() {
    let client = HttpClient::builder_fake(
        FakeHandler::from_http_error(|_req| HttpError::validation("boom")),
        FakeDeps::default(),
    )
    .build();

    let error = client.get("https://example.com/v1/items").fetch().await.unwrap_err();

    let info = error.request_info().expect("RequestInfo should be attached to the error");

    assert_eq!(
        info.original_uri.as_ref().unwrap().to_string().declassify_ref(),
        "https://example.com/v1/items"
    );
    // The dispatch handler attaches the live request metadata, which has been
    // populated with the resolved URI and attempt by the time it errors.
    assert!(info.routed_uri.is_some(), "dispatch error should carry the routed URI");
    assert!(info.attempt.is_some(), "dispatch error should carry the attempt");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn request_info_flows_to_router_resolution_error() {
    // A `Fail` conflict policy rejects the request before it reaches the
    // pipeline, so the error originates in the client's routing step rather than
    // the dispatch handler. The request metadata must still be attached.
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
        .router(Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail))
        .build();

    let error = client.get("https://existing.example.com/items").fetch().await.unwrap_err();

    let info = error
        .request_info()
        .expect("RequestInfo should be attached to a router-resolution error");
    assert_eq!(
        info.original_uri.as_ref().unwrap().to_string().declassify_ref(),
        "https://existing.example.com/items"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn request_info_flows_to_response_timeout_error() {
    // A response timeout fires in the client itself (outside the pipeline), so
    // the timeout error is synthesized there; it must still carry the request
    // metadata captured before the pipeline consumed the request.
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps { clock })
        .minimal_pipeline()
        .build();

    let error = client
        .get("https://example.com/v1/items")
        .response_timeout(Duration::from_secs(10))
        .fetch()
        .await
        .unwrap_err();

    let info = error
        .request_info()
        .expect("RequestInfo should be attached to a response-timeout error");
    assert_eq!(
        info.original_uri.as_ref().unwrap().to_string().declassify_ref(),
        "https://example.com/v1/items"
    );
    // The metadata is captured after routing, so the resolved URI is present.
    assert!(info.routed_uri.is_some(), "response-timeout error should carry the routed URI");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn request_info_flows_to_internal_pipeline_timeout_error() {
    // The standard pipeline's own total/attempt timeout layers synthesize a
    // timeout error without request context. The client backfills the captured
    // metadata so it still reaches the caller.
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps { clock }).build();

    let error = client.get("https://example.com/v1/items").fetch().await.unwrap_err();

    let info = error
        .request_info()
        .expect("RequestInfo should be backfilled onto an internal pipeline timeout error");
    assert_eq!(
        info.original_uri.as_ref().unwrap().to_string().declassify_ref(),
        "https://example.com/v1/items"
    );
    // Routing ran before the pipeline, so the backfilled metadata also carries
    // the resolved URI.
    assert!(info.routed_uri.is_some(), "routed_uri should be present in the backfilled metadata");
}
