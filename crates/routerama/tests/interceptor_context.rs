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
    assert_eq!(context.extension::<UserId>(), Some(&UserId(42)));
    context.headers_mut().insert("x-checked", "1".parse().expect("valid header"));
    assert_eq!(context.remove_extension::<UserId>(), Some(UserId(42)));
    assert_eq!(context.extension::<UserId>(), None);

    assert_eq!(parts.headers["x-checked"], "1");
}

#[test]
fn before_context_exposes_mutable_method_uri_version_extensions_and_whole_parts() {
    let (mut parts, ()) = Request::builder()
        .method("GET")
        .uri("/widgets/7")
        .body(())
        .expect("static request metadata is valid")
        .into_parts();

    let mut context = BeforeContext::new(&mut parts);
    *context.method_mut() = Method::POST;
    assert_eq!(context.method(), Method::POST);

    *context.uri_mut() = "/widgets/9".parse().expect("valid uri");
    assert_eq!(context.uri().path(), "/widgets/9");

    assert_eq!(context.version(), Version::HTTP_11);

    assert!(context.extensions().get::<u32>().is_none());
    context.extensions_mut().insert(7_u32);
    assert_eq!(context.extensions().get::<u32>(), Some(&7));

    assert_eq!(context.parts().method, Method::POST);
    context.parts_mut().method = Method::DELETE;

    assert_eq!(parts.method, Method::DELETE);
    assert_eq!(parts.uri.path(), "/widgets/9");
    assert_eq!(parts.extensions.get::<u32>(), Some(&7));
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
    assert_eq!(context.extension::<UserId>(), Some(&UserId(3)));
    context.headers_mut().insert("x-guarded", "1".parse().expect("valid header"));
    assert_eq!(context.remove_extension::<UserId>(), Some(UserId(3)));

    // The extensions field is directly readable/writable too, distinct from
    // the typed insert/get/remove helpers exercised above.
    assert!(context.extensions().get::<u32>().is_none());
    context.extensions_mut().insert(11_u32);
    assert_eq!(context.extensions().get::<u32>(), Some(&11));

    // The capture is still live after the guard mutated headers and
    // extensions, which is the borrow the split context preserves.
    assert_eq!(capture, "rust-in-action");
    assert_eq!(parts.headers["x-guarded"], "1");
    assert_eq!(parts.extensions.get::<u32>(), Some(&11));
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
fn after_context_exposes_status_mut_version_and_typed_extensions() {
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TraceId(u32);

    let (request_parts, ()) = Request::builder()
        .uri("/status")
        .body(())
        .expect("static request metadata is valid")
        .into_parts();
    let (mut response_parts, _body) = Response::new(Body::empty()).into_parts();
    response_parts.status = StatusCode::OK;

    let mut context = AfterContext::new(&request_parts, &mut response_parts);
    *context.status_mut() = StatusCode::ACCEPTED;
    assert_eq!(context.status(), StatusCode::ACCEPTED);
    assert_eq!(context.version(), Version::HTTP_11);

    assert!(context.extensions().get::<u32>().is_none());
    context.extensions_mut().insert(5_u32);
    assert_eq!(context.extensions().get::<u32>(), Some(&5));

    assert_eq!(context.insert_extension(TraceId(1)), None);
    assert_eq!(context.extension::<TraceId>(), Some(&TraceId(1)));
    assert_eq!(context.remove_extension::<TraceId>(), Some(TraceId(1)));
    assert_eq!(context.extension::<TraceId>(), None);

    assert_eq!(response_parts.status, StatusCode::ACCEPTED);
    assert_eq!(response_parts.extensions.get::<u32>(), Some(&5));
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
