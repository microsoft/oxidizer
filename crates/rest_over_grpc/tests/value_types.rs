// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the always-available value types: [`Code`],
//! [`Status`], and [`HttpResponse`].

use http::StatusCode;
use rest_over_grpc::handling::{Code, Status};
use rest_over_grpc::transcoding::HttpResponse;

#[test]
fn code_i32_round_trips() {
    for value in 0..=16 {
        let code = Code::from_i32(value).expect("0..=16 are valid codes");
        assert_eq!(code.as_i32(), value);
    }
    assert_eq!(Code::from_i32(17), None);
    assert_eq!(Code::from_i32(-1), None);
}

#[test]
fn i32_from_code_matches_as_i32() {
    assert_eq!(i32::from(Code::NotFound), 5);
    assert_eq!(i32::from(Code::Ok), 0);
}

#[test]
fn code_display_renders_canonical_names() {
    assert_eq!(Code::Ok.to_string(), "OK");
    assert_eq!(Code::InvalidArgument.to_string(), "INVALID_ARGUMENT");
    assert_eq!(Code::Unauthenticated.to_string(), "UNAUTHENTICATED");
    for value in 0..=16 {
        let name = Code::from_i32(value).expect("0..=16 are valid codes").to_string();
        assert!(!name.is_empty());
        assert_eq!(name, name.to_uppercase());
    }
}

#[test]
fn try_from_i32_round_trips_and_reports_unknown() {
    assert_eq!(Code::try_from(5), Ok(Code::NotFound));
    let error = Code::try_from(999).expect_err("999 is not a canonical code");
    assert_eq!(error.value(), 999);
    assert_eq!(error, Code::try_from(999).unwrap_err());
    assert_eq!(error.to_string(), "unknown gRPC status code: 999");
}

#[test]
fn code_forward_mapping_known_values() {
    assert_eq!(Code::Ok.to_http_status(), StatusCode::OK);
    assert_eq!(Code::NotFound.to_http_status(), StatusCode::NOT_FOUND);
    assert_eq!(Code::AlreadyExists.to_http_status(), StatusCode::CONFLICT);
    assert_eq!(Code::Aborted.to_http_status(), StatusCode::CONFLICT);
    assert_eq!(Code::PermissionDenied.to_http_status(), StatusCode::FORBIDDEN);
    assert_eq!(Code::Unauthenticated.to_http_status(), StatusCode::UNAUTHORIZED);
    assert_eq!(Code::ResourceExhausted.to_http_status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(Code::Unimplemented.to_http_status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(Code::DeadlineExceeded.to_http_status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(Code::Cancelled.to_http_status().as_u16(), 499);
}

#[test]
fn code_forward_mapping_covers_remaining_arms() {
    assert_eq!(Code::Unknown.to_http_status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(Code::Internal.to_http_status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(Code::DataLoss.to_http_status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(Code::InvalidArgument.to_http_status(), StatusCode::BAD_REQUEST);
    assert_eq!(Code::FailedPrecondition.to_http_status(), StatusCode::BAD_REQUEST);
    assert_eq!(Code::OutOfRange.to_http_status(), StatusCode::BAD_REQUEST);
    assert_eq!(Code::Unavailable.to_http_status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn code_reverse_mapping_known_values() {
    assert_eq!(Code::from_http_status(StatusCode::OK), Code::Ok);
    assert_eq!(Code::from_http_status(StatusCode::CREATED), Code::Ok);
    assert_eq!(Code::from_http_status(StatusCode::BAD_REQUEST), Code::Internal);
    assert_eq!(Code::from_http_status(StatusCode::UNAUTHORIZED), Code::Unauthenticated);
    assert_eq!(Code::from_http_status(StatusCode::FORBIDDEN), Code::PermissionDenied);
    assert_eq!(Code::from_http_status(StatusCode::NOT_FOUND), Code::Unimplemented);
    assert_eq!(Code::from_http_status(StatusCode::SERVICE_UNAVAILABLE), Code::Unavailable);
    assert_eq!(Code::from_http_status(StatusCode::INTERNAL_SERVER_ERROR), Code::Unknown);
}

#[test]
fn status_accessors() {
    let s = Status::invalid_argument("bad shelf id");
    assert_eq!(s.code(), Code::InvalidArgument);
    assert_eq!(s.message(), "bad shelf id");
    assert!(s.to_string().contains("bad shelf id"));
}

#[test]
fn response_accessors() {
    let r = HttpResponse::ok_json(b"{}".to_vec());
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.content_type(), "application/json");
    assert_eq!(r.body(), b"{}");
    assert_eq!(r.into_body(), b"{}");
}

#[test]
fn into_http_falls_back_on_invalid_content_type() {
    // A content type with control bytes can't be set as a header value, so
    // `into_http` takes its bare-response fallback while preserving the status.
    let r = HttpResponse::new(StatusCode::IM_A_TEAPOT, "bad\nvalue", b"body".to_vec());
    let http = r.into_http();
    assert_eq!(http.status(), StatusCode::IM_A_TEAPOT);
    assert!(http.headers().get(http::header::CONTENT_TYPE).is_none());
    assert!(http.body().is_empty());
}

#[test]
fn from_conversions_match_inherent_methods() {
    let bytes = Vec::<u8>::from(HttpResponse::ok_json(b"{}".to_vec()));
    assert_eq!(bytes, b"{}");

    let http = http::Response::<Vec<u8>>::from(HttpResponse::ok_json(b"[]".to_vec()));
    assert_eq!(http.status(), StatusCode::OK);
    assert_eq!(http.headers()[http::header::CONTENT_TYPE], "application/json");
}

#[test]
fn from_status_and_not_found_build_error_responses() {
    let response = HttpResponse::from_status(&Status::not_found("gone"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let value: serde_json::Value = serde_json::from_slice(response.body()).expect("valid json");
    assert_eq!(value["code"], Code::NotFound.as_i32());
    assert_eq!(value["message"], "gone");

    let not_found = HttpResponse::not_found();
    assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
}

#[test]
fn from_status_renders_details_and_omits_them_when_empty() {
    let plain = HttpResponse::from_status(&Status::invalid_argument("bad"));
    let value: serde_json::Value = serde_json::from_slice(plain.body()).expect("valid json");
    assert!(value.get("details").is_none(), "empty details must be omitted");

    let detailed = Status::invalid_argument("bad").with_details([
        serde_json::json!({ "@type": "type.googleapis.com/google.rpc.BadRequest", "field": "shelf" }),
        serde_json::json!({ "reason": "NON_NUMERIC" }),
    ]);
    let response = HttpResponse::from_status(&detailed);
    let value: serde_json::Value = serde_json::from_slice(response.body()).expect("valid json");
    assert_eq!(value["code"], Code::InvalidArgument.as_i32());
    assert_eq!(value["message"], "bad");
    assert_eq!(value["details"].as_array().expect("details is an array").len(), 2);
    assert_eq!(value["details"][1]["reason"], "NON_NUMERIC");
}

#[test]
fn into_http_appends_custom_headers_and_keeps_content_type_authoritative() {
    let response = HttpResponse::ok_json(b"{}".to_vec())
        .with_header(http::header::LOCATION, http::HeaderValue::from_static("/v1/shelves/7"))
        // A stray custom content-type must not survive alongside the negotiated one.
        .with_header(http::header::CONTENT_TYPE, http::HeaderValue::from_static("text/plain"));

    let http = response.into_http();
    assert_eq!(http.headers()[http::header::LOCATION], "/v1/shelves/7");
    assert_eq!(http.headers().get_all(http::header::CONTENT_TYPE).iter().count(), 1);
    assert_eq!(http.headers()[http::header::CONTENT_TYPE], "application/json");
}

#[test]
fn merge_headers_preserves_existing_and_repeated_values() {
    let mut response = HttpResponse::ok_json(b"{}".to_vec());
    response
        .headers_mut()
        .append(http::header::SET_COOKIE, http::HeaderValue::from_static("a=1"));

    let mut extra = http::HeaderMap::new();
    extra.append(http::header::SET_COOKIE, http::HeaderValue::from_static("b=2"));
    extra.append(http::header::SET_COOKIE, http::HeaderValue::from_static("c=3"));
    response.merge_headers(extra);

    let http = response.into_http();
    assert_eq!(http.headers().get_all(http::header::SET_COOKIE).iter().count(), 3);
}
