// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end transcoding tests: real `pbjson` messages flowing through the
//! generated REST router, service trait, and dispatcher.

use rest_over_grpc_sample::InMemoryLibrary;
use rest_over_grpc_sample::service::dispatch;
use serde_json::Value;

fn body(value: &rest_over_grpc::HttpResponse) -> Value {
    serde_json::from_slice(value.body()).expect("response body is valid JSON")
}

#[tokio::test]
async fn get_shelf_binds_path_variable() {
    let resp = dispatch(&InMemoryLibrary, "GET", "/v1/shelves/history", b"").await;
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    assert_eq!(json["name"], "shelves/history");
    assert_eq!(json["theme"], "history");
}

#[tokio::test]
async fn create_shelf_binds_body_field() {
    let payload = br#"{"name":"ignored","theme":"sci-fi"}"#;
    let resp = dispatch(&InMemoryLibrary, "POST", "/v1/shelves", payload).await;
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    assert_eq!(json["name"], "shelves/created");
    assert_eq!(json["theme"], "sci-fi");
}

#[tokio::test]
async fn list_shelves_binds_query_parameter() {
    let resp = dispatch(&InMemoryLibrary, "GET", "/v1/shelves?filter=science", b"").await;
    assert_eq!(resp.status().as_u16(), 200);
    let json = body(&resp);
    let shelves = json["shelves"].as_array().expect("array");
    assert_eq!(shelves.len(), 1);
    assert_eq!(shelves[0]["theme"], "science");
}

#[tokio::test]
async fn unknown_route_yields_404() {
    let resp = dispatch(&InMemoryLibrary, "GET", "/v1/widgets/1", b"").await;
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn handler_status_maps_to_http() {
    let resp = dispatch(&InMemoryLibrary, "GET", "/v1/shelves/missing", b"").await;
    assert_eq!(resp.status().as_u16(), 404);
    let json = body(&resp);
    assert_eq!(json["message"], "no such shelf");
}

#[tokio::test]
async fn method_disambiguates_same_path() {
    let get = dispatch(&InMemoryLibrary, "GET", "/v1/shelves", b"").await;
    assert!(body(&get)["shelves"].is_array());

    let payload = br#"{"theme":"poetry"}"#;
    let post = dispatch(&InMemoryLibrary, "POST", "/v1/shelves", payload).await;
    assert_eq!(body(&post)["theme"], "poetry");
}
