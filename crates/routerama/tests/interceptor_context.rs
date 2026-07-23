// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Interceptor context types exercised through the public `route` API.

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode, Version};
use routerama::response::Body;
use routerama::route::{AfterContext, Before, BeforeContext, BodyConsumed, BodyTransform, SelectedContext};

#[test]
fn before_context_enriches_and_reads_request_metadata() {
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct UserId(u32);

    let (mut parts, ()) = Request::builder()
        .method("PATCH")
        .uri("/widgets/7")
        .header("x-api-key", "secret")
        .body(())
        .expect("static request metadata is valid")
        .into_parts();

    let mut context = BeforeContext::new(&mut parts);
    assert_eq!(context.method(), Method::PATCH);
    assert_eq!(context.uri().path(), "/widgets/7");
    assert_eq!(context.headers()["x-api-key"], "secret");
    assert_eq!(context.insert_extension(UserId(42)), None);
    assert_eq!(context.get_extension::<UserId>(), Some(&UserId(42)));
    context.headers_mut().insert("x-checked", "1".parse().expect("valid header"));
    assert_eq!(context.remove_extension::<UserId>(), Some(UserId(42)));
    assert_eq!(context.get_extension::<UserId>(), None);

    assert_eq!(parts.headers["x-checked"], "1");
}

#[test]
fn selected_context_coexists_with_a_borrowed_uri_capture() {
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct UserId(u32);

    let (mut parts, ()) = Request::builder()
        .uri("/books/rust-in-action")
        .header("x-api-key", "secret")
        .body(())
        .expect("static request metadata is valid")
        .into_parts();

    // A zero-copy capture borrowed from the URI, exactly like the one a
    // selected route hands to a handler taking `&str`.
    let capture: &str = parts.uri.path().rsplit('/').next().expect("the path has a final segment");

    let mut context = SelectedContext::new(&parts.method, &parts.uri, parts.version, &mut parts.headers, &mut parts.extensions);
    assert_eq!(context.method(), Method::GET);
    assert_eq!(context.uri().path(), "/books/rust-in-action");
    assert_eq!(context.version(), Version::HTTP_11);
    assert_eq!(context.headers()["x-api-key"], "secret");
    assert_eq!(context.insert_extension(UserId(3)), None);
    assert_eq!(context.get_extension::<UserId>(), Some(&UserId(3)));
    context.headers_mut().insert("x-guarded", "1".parse().expect("valid header"));
    assert_eq!(context.remove_extension::<UserId>(), Some(UserId(3)));

    // The capture is still live after the guard mutated headers and
    // extensions, which is the borrow the split context preserves.
    assert_eq!(capture, "rust-in-action");
    assert_eq!(parts.headers["x-guarded"], "1");
}

#[test]
fn after_context_mutates_response_and_reads_request() {
    let (request_parts, ()) = Request::builder()
        .uri("/status")
        .body(())
        .expect("static request metadata is valid")
        .into_parts();
    let (mut response_parts, _body) = Response::new(Body::empty()).into_parts();

    let mut context = AfterContext::new(&request_parts, &mut response_parts);
    assert_eq!(context.request().uri.path(), "/status");
    context.set_status(StatusCode::CREATED);
    context.headers_mut().insert("x-trace", "abc".parse().expect("valid header"));

    assert_eq!(response_parts.status, StatusCode::CREATED);
    assert_eq!(response_parts.headers["x-trace"], "abc");
}

#[test]
fn control_flow_outcomes_are_constructible() {
    let next: Before<StatusCode> = Before::Next;
    assert_eq!(next, Before::Next);

    let replace: BodyTransform<Body, StatusCode> = BodyTransform::Replace(Body::from_bytes(Bytes::from_static(b"hi")));
    assert!(matches!(replace, BodyTransform::Replace(_)));

    let consumed: BodyConsumed<StatusCode> = BodyConsumed::Consumed;
    assert_eq!(consumed, BodyConsumed::Consumed);
}
