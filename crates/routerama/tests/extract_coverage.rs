// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for request-parts/body extraction wrappers and route failures.

use std::collections::VecDeque;
use std::error::Error as _;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::Extensions;
use http::header::CONTENT_TYPE;
use http_body::{Frame, SizeHint};
use routerama::query::FromQuery;
use routerama::response::IntoResponse as _;
use routerama::route::__private::{ResolveError, route_failure};
use routerama::route::{
    BodyRejection, BodyTransportError, BytesBody, ClonedExtension, ExtensionRef, FromRequestBody, FromRequestParts, MissingExtension,
    Query, QueryRejection, RawBody, Request, RouteFailure, State, StatusCode, TextBody,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TestBodyError(&'static str);

impl std::fmt::Display for TestBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for TestBodyError {}

struct TestBody {
    frames: VecDeque<Result<Frame<Bytes>, TestBodyError>>,
}

impl TestBody {
    fn from_chunks(chunks: Vec<Bytes>) -> Self {
        Self {
            frames: chunks.into_iter().map(|chunk| Ok(Frame::data(chunk))).collect(),
        }
    }

    fn failed(error: &'static str) -> Self {
        Self {
            frames: [Err(TestBodyError(error))].into(),
        }
    }

    fn empty_frames(count: usize) -> Self {
        Self {
            frames: core::iter::repeat_with(|| Ok(Frame::data(Bytes::new()))).take(count).collect(),
        }
    }
}

impl http_body::Body for TestBody {
    type Data = Bytes;
    type Error = TestBodyError;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frames.pop_front())
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn parts() -> http::request::Parts {
    Request::post("/")
        .body(())
        .expect("the test request uses valid static metadata")
        .into_parts()
        .0
}

#[derive(Debug)]
struct Marker;

#[derive(Debug, FromQuery, PartialEq, Eq)]
struct Search {
    term: String,
}

#[tokio::test(flavor = "current_thread")]
async fn state_extension_and_missing_extension_wrappers_expose_their_inner_values() {
    let mut parts = parts();
    parts.extensions.insert(42_i32);

    let state_wrapper = <State<i64> as FromRequestParts<'_, i64>>::from_request_parts(&parts, &7_i64).expect("state extraction succeeds");
    assert_eq!(*state_wrapper, 7);
    assert_eq!(state_wrapper.into_inner(), 7);

    let extension_ref = <ExtensionRef<'_, i32> as FromRequestParts<'_, ()>>::from_request_parts(&parts, &()).expect("extension is present");
    assert_eq!(*extension_ref.get(), 42);
    assert_eq!(*extension_ref, 42);
    assert_eq!(*extension_ref.into_inner(), 42);

    let cloned = <ClonedExtension<i32> as FromRequestParts<'_, ()>>::from_request_parts(&parts, &()).expect("extension is present");
    assert_eq!(*cloned, 42);
    assert_eq!(cloned.into_inner(), 42);

    let empty_extensions = Extensions::new();
    let mut missing_parts = parts.clone();
    missing_parts.extensions = empty_extensions;
    let missing =
        <ExtensionRef<'_, Marker> as FromRequestParts<'_, ()>>::from_request_parts(&missing_parts, &()).expect_err("no marker present");
    assert_eq!(
        missing.to_string(),
        format!("request extension `{}` is missing", core::any::type_name::<Marker>())
    );
    assert!(format!("{missing:?}").contains("MissingExtension"));
    let defaulted = MissingExtension::<Marker>::default();
    assert_eq!(defaulted.to_string(), missing.to_string());
    assert_eq!(missing.into_response().status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test(flavor = "current_thread")]
async fn bytes_text_and_raw_bodies_buffer_and_deref_their_contents() {
    let parts = parts();

    let bytes_body = <BytesBody<16> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"hello")]),
        &(),
    )
    .await
    .expect("body fits the limit");
    assert_eq!(bytes_body.as_bytes(), b"hello");
    assert_eq!(&*bytes_body, &Bytes::from_static(b"hello"));
    assert_eq!(bytes_body.into_inner(), Bytes::from_static(b"hello"));

    let text_body = <TextBody<16> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"hello")]),
        &(),
    )
    .await
    .expect("body fits the limit");
    assert_eq!(text_body.as_str(), "hello");
    assert_eq!(&*text_body, "hello");
    assert_eq!(text_body.into_inner(), "hello".to_owned());

    let raw_body = <RawBody<Bytes> as FromRequestBody<(), Bytes>>::from_request_body(&parts, Bytes::from_static(b"raw"), &())
        .await
        .expect("raw body extraction is infallible");
    assert_eq!(&*raw_body, &Bytes::from_static(b"raw"));
    assert_eq!(raw_body.into_inner(), Bytes::from_static(b"raw"));
}

#[tokio::test(flavor = "current_thread")]
async fn body_rejection_variants_render_and_expose_their_source() {
    let parts = parts();

    let too_large = <BytesBody<2> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"abc")]),
        &(),
    )
    .await
    .expect_err("body exceeds the byte limit");
    assert!(matches!(too_large, BodyRejection::TooLarge(_)));
    assert!(!too_large.to_string().is_empty());
    assert!(too_large.source().is_some());

    let too_many_frames = <BytesBody<0> as FromRequestBody<(), TestBody>>::from_request_body(&parts, TestBody::empty_frames(65), &())
        .await
        .expect_err("body exceeds its frame budget");
    assert!(matches!(too_many_frames, BodyRejection::TooManyFrames(_)));
    assert!(!too_many_frames.to_string().is_empty());
    assert!(too_many_frames.source().is_some());

    let transport = <BytesBody<16> as FromRequestBody<(), TestBody>>::from_request_body(&parts, TestBody::failed("diagnostic"), &())
        .await
        .expect_err("body transport fails");
    assert!(matches!(transport, BodyRejection::Transport(_)));
    assert!(transport.to_string().contains("diagnostic"));
    assert!(transport.source().is_some());

    let invalid_utf8 = <TextBody<16> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"\xff")]),
        &(),
    )
    .await
    .expect_err("body is not valid UTF-8");
    assert!(matches!(invalid_utf8, BodyRejection::InvalidUtf8(_)));
    assert!(!invalid_utf8.to_string().is_empty());
    assert!(invalid_utf8.source().is_some());

    let transport_error = BodyTransportError::new(TestBodyError("standalone"));
    assert_eq!(transport_error.to_string(), "request body transport failed: standalone");
    assert!(transport_error.source().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn query_wrapper_and_rejection_expose_their_inner_values() {
    let request = Request::get("/?term=rust")
        .body(())
        .expect("the test request uses valid static metadata");
    let (parts, ()) = request.into_parts();

    let query = <Query<Search> as FromRequestParts<'_, ()>>::from_request_parts(&parts, &()).expect("query decodes successfully");
    assert_eq!(query.term, "rust");
    assert_eq!(query.into_inner(), Search { term: "rust".to_owned() });

    let bad_request = Request::get("/?term=%GG")
        .body(())
        .expect("the test request uses valid static metadata");
    let (bad_parts, ()) = bad_request.into_parts();
    let rejection: QueryRejection =
        <Query<Search> as FromRequestParts<'_, ()>>::from_request_parts(&bad_parts, &()).expect_err("malformed percent-encoding fails");
    assert_eq!(rejection.to_string(), rejection.error().to_string());
    assert_eq!(rejection.into_response().status(), StatusCode::BAD_REQUEST);
}

// Confirms the `CONTENT_TYPE` header import is used, keeping this module's
// imports aligned with the other extraction coverage test files.
#[test]
fn content_type_header_name_is_the_expected_constant() {
    assert_eq!(CONTENT_TYPE.as_str(), "content-type");
}

#[test]
fn route_failure_status_path_and_field_accessors_cover_every_variant() {
    let not_found = RouteFailure::NotFound { path: "/missing" };
    assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
    assert_eq!(not_found.path(), Some("/missing"));
    assert_eq!(not_found.field(), None);

    let malformed_path = RouteFailure::MalformedPath { path: "/bad?query" };
    assert_eq!(malformed_path.status(), StatusCode::BAD_REQUEST);
    assert_eq!(malformed_path.path(), Some("/bad?query"));
    assert_eq!(malformed_path.field(), None);

    let missing_capture = RouteFailure::MissingCapture { field: "id" };
    assert_eq!(missing_capture.status(), StatusCode::BAD_REQUEST);
    assert_eq!(missing_capture.path(), None);
    assert_eq!(missing_capture.field(), Some("id"));

    let invalid_capture = RouteFailure::InvalidCapture { field: "id" };
    assert_eq!(invalid_capture.status(), StatusCode::BAD_REQUEST);
    assert_eq!(invalid_capture.path(), None);
    assert_eq!(invalid_capture.field(), Some("id"));

    let undecodable_capture = RouteFailure::UndecodableCapture { field: "id" };
    assert_eq!(undecodable_capture.status(), StatusCode::BAD_REQUEST);
    assert_eq!(undecodable_capture.path(), None);
    assert_eq!(undecodable_capture.field(), Some("id"));

    let host_mismatch = RouteFailure::HostMismatch { path: "/host" };
    assert_eq!(host_mismatch.status(), StatusCode::NOT_FOUND);
    assert_eq!(host_mismatch.path(), Some("/host"));
    assert_eq!(host_mismatch.field(), None);

    let unsupported_media_type = RouteFailure::UnsupportedMediaType { path: "/media" };
    assert_eq!(unsupported_media_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(unsupported_media_type.path(), Some("/media"));
    assert_eq!(unsupported_media_type.field(), None);

    let not_acceptable = RouteFailure::NotAcceptable { path: "/accept" };
    assert_eq!(not_acceptable.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(not_acceptable.path(), Some("/accept"));
    assert_eq!(not_acceptable.field(), None);

    for failure in [
        not_found,
        malformed_path,
        missing_capture,
        invalid_capture,
        undecodable_capture,
        host_mismatch,
        unsupported_media_type,
        not_acceptable,
    ] {
        assert!(!failure.to_string().is_empty());
        assert!(failure.source().is_none());
        let status = failure.status();
        assert_eq!(failure.into_response().status(), status);
    }
}

#[test]
fn route_failure_is_produced_from_every_resolve_error_variant() {
    let not_found = route_failure(ResolveError::NotFound("/missing"));
    assert!(matches!(not_found, RouteFailure::NotFound { path: "/missing" }));

    let invalid_path = route_failure(ResolveError::InvalidPath("/bad?query"));
    assert!(matches!(invalid_path, RouteFailure::MalformedPath { path: "/bad?query" }));

    let missing_capture = route_failure(ResolveError::MissingCapture("id"));
    assert!(matches!(missing_capture, RouteFailure::MissingCapture { field: "id" }));

    let invalid_capture = route_failure(ResolveError::InvalidCapture("id"));
    assert!(matches!(invalid_capture, RouteFailure::InvalidCapture { field: "id" }));

    let undecodable_capture = route_failure(ResolveError::UndecodableCapture("id"));
    assert!(matches!(undecodable_capture, RouteFailure::UndecodableCapture { field: "id" }));
}
