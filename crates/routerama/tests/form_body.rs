// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public-API coverage for the form response body.

use std::error::Error as _;

use http::StatusCode;
use http::header::CONTENT_TYPE;
use http_body_util::BodyExt as _;
use routerama::query::{QueryLimits, ToQuery};
use routerama::response::IntoResponse;
use routerama::route::form::{FormBody, FormEncodeError};

#[derive(ToQuery)]
struct Paging {
    page: usize,
    filter: String,
}

#[test]
fn new_percent_encodes_reserved_characters() {
    let body = FormBody::new(&Paging {
        page: 3,
        filter: "a b&c".to_owned(),
    })
    .unwrap();

    assert_eq!(body.as_str(), "page=3&filter=a+b%26c");
}

#[test]
fn into_string_returns_the_encoded_form() {
    let body = FormBody::new(&Paging {
        page: 1,
        filter: String::new(),
    })
    .unwrap();

    assert_eq!(body.into_string(), "page=1&filter=");
}

#[tokio::test]
async fn into_response_sets_the_form_media_type_and_a_200_status() {
    let response = FormBody::new(&Paging {
        page: 7,
        filter: "x".to_owned(),
    })
    .unwrap()
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/x-www-form-urlencoded");

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes, b"page=7&filter=x"[..]);
}

#[test]
fn new_with_reports_an_exceeded_output_limit() {
    let limits = QueryLimits::DEFAULT.with_max_encoded_length(4);

    let error = FormBody::new_with(
        &Paging {
            page: 3,
            filter: "a long value".to_owned(),
        },
        limits,
    )
    .unwrap_err();

    assert!(error.source().is_some());
    assert!(!error.to_string().is_empty());
    _ = error.error();
    _ = error.into_inner();
}

#[tokio::test]
async fn an_encoding_failure_becomes_an_empty_500() {
    let limits = QueryLimits::DEFAULT.with_max_encoded_length(4);
    let error: FormEncodeError = FormBody::new_with(
        &Paging {
            page: 3,
            filter: "a long value".to_owned(),
        },
        limits,
    )
    .unwrap_err();

    let response = error.into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get(CONTENT_TYPE).is_none());

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn a_result_handler_return_selects_the_success_branch() {
    let reply: Result<FormBody, FormEncodeError> = FormBody::new(&Paging {
        page: 2,
        filter: "ok".to_owned(),
    });
    let response = reply.into_response();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/x-www-form-urlencoded");

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes, b"page=2&filter=ok"[..]);
}
