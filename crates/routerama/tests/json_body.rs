// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public-API coverage for the JSON response body.

use std::collections::BTreeMap;
use std::error::Error as _;

use http::StatusCode;
use http::header::CONTENT_TYPE;
use http_body_util::BodyExt as _;
use routerama::response::IntoResponse;
use routerama::route::json::{JsonBody, JsonEncodeError};
use serde::Serialize;
use serde::ser::Serializer;

#[derive(Serialize)]
struct Reply {
    title: String,
    tags: usize,
}

/// A value whose `Serialize` implementation always fails.
struct Unserializable;

impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("this value never serializes"))
    }
}

#[test]
fn new_serializes_the_value_without_a_trailing_newline() {
    let body = JsonBody::new(&Reply {
        title: "routing".to_owned(),
        tags: 2,
    })
    .unwrap();

    assert_eq!(body.as_bytes(), &br#"{"title":"routing","tags":2}"#[..]);
}

#[test]
fn new_escapes_embedded_quotes() {
    let body = JsonBody::new(&Reply {
        title: r#"quoted "value""#.to_owned(),
        tags: 0,
    })
    .unwrap();

    assert_eq!(body.as_bytes(), &br#"{"title":"quoted \"value\"","tags":0}"#[..]);
}

#[test]
fn into_bytes_returns_the_serialized_json() {
    let body = JsonBody::new(&[1_u8, 2, 3]).unwrap();

    assert_eq!(body.into_bytes(), &b"[1,2,3]"[..]);
}

#[tokio::test]
async fn into_response_sets_the_json_media_type_and_a_200_status() {
    let response = JsonBody::new(&Reply {
        title: "routing".to_owned(),
        tags: 0,
    })
    .unwrap()
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes, br#"{"title":"routing","tags":0}"#[..]);
}

#[test]
fn new_reports_a_failing_serialize_implementation() {
    let error = JsonBody::new(&Unserializable).unwrap_err();

    assert!(error.to_string().contains("this value never serializes"));
    assert!(error.source().is_some());
    assert!(error.error().is_data());
}

#[test]
fn new_reports_a_non_string_map_key() {
    let mut map = BTreeMap::new();
    _ = map.insert(vec![1_u8], "value");

    let error = JsonBody::new(&map).unwrap_err();

    assert!(error.to_string().contains("key must be a string"));
    assert_ne!(error.into_inner().classify(), serde_json::error::Category::Eof);
}

#[tokio::test]
async fn an_encoding_failure_becomes_an_empty_500() {
    let error: JsonEncodeError = JsonBody::new(&Unserializable).unwrap_err();
    let response = error.into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get(CONTENT_TYPE).is_none());

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn a_result_handler_return_selects_the_success_branch() {
    let reply: Result<JsonBody, JsonEncodeError> = JsonBody::new(&Reply {
        title: "ok".to_owned(),
        tags: 1,
    });
    let response = reply.into_response();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes, br#"{"title":"ok","tags":1}"#[..]);
}

#[tokio::test]
async fn a_result_handler_return_selects_the_failure_branch() {
    let reply: Result<JsonBody, JsonEncodeError> = JsonBody::new(&Unserializable);
    let response = reply.into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.is_empty());
}
