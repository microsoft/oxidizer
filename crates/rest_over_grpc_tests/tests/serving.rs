// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end tests of the `http`/`tower` adapter driving the generated
//! transcoder, as a real server would.

#![cfg(not(miri))] // the tokio runtime is unsupported under Miri.

use std::sync::Arc;

use bytes::Bytes;
use http::{Method, Request};
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::serving::{RestBody, RestService, serve_http};
use rest_over_grpc_tests::custom::{InMemoryLibrary, Transcoder};
use serde_json::Value;
use tower_service::Service as _;

/// Parses an adapter response body ([`RestBody`], an [`http_body::Body`]) as
/// JSON, collecting it before parsing.
async fn body_json(response: http::Response<RestBody>) -> Value {
    let bytes = response.into_body().collect().await.expect("body collects").to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn serve_http_end_to_end() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/shelves/history")
        .body(Full::new(Bytes::new()))
        .expect("valid request");

    let response = serve_http(request, &Transcoder::new(InMemoryLibrary)).await;
    assert_eq!(response.status().as_u16(), 200);
    let json = body_json(response).await;
    assert_eq!(json["name"], "shelves/history");
}

#[tokio::test]
async fn tower_service_with_query_and_body() {
    // `Arc<Transcoder>` implements `Transcode`, so a single shared instance is
    // cloned cheaply into the service on each request.
    let mut service = RestService::new(Arc::new(Transcoder::new(InMemoryLibrary)));

    // GET with a query parameter.
    let list = Request::builder()
        .method(Method::GET)
        .uri("/v1/shelves?filter=science")
        .body(Full::new(Bytes::new()))
        .expect("valid request");
    let response = service.call(list).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 200);
    let json = body_json(response).await;
    assert_eq!(json["shelves"].as_array().expect("array").len(), 1);

    // POST with a JSON body mapped to the `shelf` field.
    let create = Request::builder()
        .method(Method::POST)
        .uri("/v1/shelves")
        .body(Full::new(Bytes::from_static(br#"{"theme":"poetry"}"#)))
        .expect("valid request");
    let response = service.call(create).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 200);
    let json = body_json(response).await;
    assert_eq!(json["theme"], "poetry");
}

#[tokio::test]
async fn adapter_reports_not_found() {
    let mut service = RestService::new(Arc::new(Transcoder::new(InMemoryLibrary)));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/widgets/1")
        .body(Full::new(Bytes::new()))
        .expect("valid request");
    let response = service.call(request).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 404);
}
