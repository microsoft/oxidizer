// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end tests of the `http`/`tower` adapter driving the generated
//! dispatcher, as a real server would.

use std::sync::Arc;

use bytes::Bytes;
use http::{Method, Request, Uri};
use http_body_util::Full;
use rest_over_grpc::adapter::{RestService, transcode_http};
use rest_over_grpc_sample::InMemoryLibrary;
use rest_over_grpc_sample::service::dispatch;
use serde_json::Value;
use tower_service::Service as _;

/// Builds a dispatcher closure that routes `(method, uri, body)` through the
/// generated `dispatch` against a shared service instance.
fn dispatcher(
    library: Arc<InMemoryLibrary>,
) -> impl Fn(Method, Uri, Bytes) -> std::pin::Pin<Box<dyn std::future::Future<Output = rest_over_grpc::HttpResponse> + Send>> + Clone {
    move |method: Method, uri: Uri, body: Bytes| {
        let library = Arc::clone(&library);
        Box::pin(async move {
            let target = uri.path_and_query().map_or("/", |pq| pq.as_str());
            dispatch(&*library, method.as_str(), target, &body).await
        })
    }
}

#[tokio::test]
async fn transcode_http_end_to_end() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/shelves/history")
        .body(Full::new(Bytes::new()))
        .expect("valid request");

    let response = transcode_http(request, dispatcher(Arc::new(InMemoryLibrary))).await;
    assert_eq!(response.status().as_u16(), 200);
    let json: Value = serde_json::from_slice(response.body()).expect("json");
    assert_eq!(json["name"], "shelves/history");
}

#[tokio::test]
async fn tower_service_with_query_and_body() {
    let mut service = RestService::new(dispatcher(Arc::new(InMemoryLibrary)));

    // GET with a query parameter.
    let list = Request::builder()
        .method(Method::GET)
        .uri("/v1/shelves?filter=science")
        .body(Full::new(Bytes::new()))
        .expect("valid request");
    let response = service.call(list).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 200);
    let json: Value = serde_json::from_slice(response.body()).expect("json");
    assert_eq!(json["shelves"].as_array().expect("array").len(), 1);

    // POST with a JSON body mapped to the `shelf` field.
    let create = Request::builder()
        .method(Method::POST)
        .uri("/v1/shelves")
        .body(Full::new(Bytes::from_static(br#"{"theme":"poetry"}"#)))
        .expect("valid request");
    let response = service.call(create).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 200);
    let json: Value = serde_json::from_slice(response.body()).expect("json");
    assert_eq!(json["theme"], "poetry");
}

#[tokio::test]
async fn adapter_reports_not_found() {
    let mut service = RestService::new(dispatcher(Arc::new(InMemoryLibrary)));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/widgets/1")
        .body(Full::new(Bytes::new()))
        .expect("valid request");
    let response = service.call(request).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 404);
}
