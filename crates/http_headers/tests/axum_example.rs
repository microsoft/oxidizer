// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Semantic coverage for the Axum example.

#![cfg(feature = "http")]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use axum::body::{Body, to_bytes};
use axum::http::header::{CACHE_CONTROL, USER_AGENT};
use axum::http::{HeaderValue, Request, StatusCode};
use tower::ServiceExt;

#[path = "../examples/axum/app.rs"]
mod app;

const CACHE_POLICY: &str = "private, max-age=60";

async fn response(request: Request<Body>) -> (StatusCode, Option<HeaderValue>, String) {
    let response = app::router().oneshot(request).await.unwrap();
    let status = response.status();
    let cache_control = response.headers().get(CACHE_CONTROL).cloned();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, cache_control, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn absent_user_agent_returns_unknown_greeting_and_private_cache_policy() {
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();

    let (status, cache_control, body) = response(request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache_control, Some(HeaderValue::from_static(CACHE_POLICY)));
    assert_eq!(body, r#"{"message":"hello from http_headers","user_agent":"unknown"}"#);
}

#[tokio::test]
async fn valid_user_agent_is_returned_with_private_cache_policy() {
    let request = Request::builder()
        .uri("/")
        .header(USER_AGENT, "example-client/1.0")
        .body(Body::empty())
        .unwrap();

    let (status, cache_control, body) = response(request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache_control, Some(HeaderValue::from_static(CACHE_POLICY)));
    assert_eq!(body, r#"{"message":"hello from http_headers","user_agent":"example-client/1.0"}"#);
}

#[tokio::test]
async fn malformed_user_agents_return_bad_request() {
    let invalid_utf8 = Request::builder()
        .uri("/")
        .header(USER_AGENT, HeaderValue::from_bytes(b"\xff").unwrap())
        .body(Body::empty())
        .unwrap();

    let (status, cache_control, body) = response(invalid_utf8).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(cache_control, None);
    assert_eq!(body, "invalid user-agent header: invalid UTF-8");

    let mut multiple_values = Request::builder().uri("/").body(Body::empty()).unwrap();
    multiple_values
        .headers_mut()
        .append(USER_AGENT, HeaderValue::from_static("client/1"));
    multiple_values
        .headers_mut()
        .append(USER_AGENT, HeaderValue::from_static("client/2"));

    let (status, cache_control, body) = response(multiple_values).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(cache_control, None);
    assert_eq!(body, "invalid user-agent header: unexpected multiple values");
}
