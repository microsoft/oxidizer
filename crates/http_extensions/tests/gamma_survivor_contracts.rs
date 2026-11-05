// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory body, builder, cloning, and routing contracts.

use futures::executor::block_on;
use http::{Method, Version};
use http_extensions::routing::{BaseUriConflict, RequestUris, Router, RouterContext};
use http_extensions::{HttpBodyBuilder, HttpBodyOptions, HttpError, HttpRequestBuilder, HttpRequestExt, HttpResponseBuilder, RequestExt};
use templated_uri::BaseUri;

fn body_builder() -> HttpBodyBuilder {
    HttpBodyBuilder::new(
        bytesbuf::mem::OpaqueMemory::new(bytesbuf::mem::GlobalPool::new()),
        &tick::Clock::new_frozen(),
    )
}

#[test]
fn gamma_survivor_clone_preserves_nondefault_protocol_and_payload() {
    let builder = body_builder();
    let mut request = HttpRequestBuilder::new(&builder)
        .method(Method::PATCH)
        .uri("https://example.test/path?q=value")
        .header("x-one", "first")
        .text("payload")
        .build()
        .unwrap();
    *request.version_mut() = Version::HTTP_2;
    request.extensions_mut().insert(73_u32);
    let clone = request.try_clone().unwrap();
    assert_eq!(clone.version(), Version::HTTP_2);
    assert_eq!(clone.method(), Method::PATCH);
    assert_eq!(clone.uri(), request.uri());
    assert_eq!(clone.headers(), request.headers());
    assert_eq!(clone.extensions().get::<u32>(), Some(&73));
    assert_eq!(block_on(clone.into_body().into_text()).unwrap(), "payload");
}

#[test]
fn gamma_survivor_builders_propagate_errors_without_panicking() {
    let builder = body_builder();
    let error = HttpRequestBuilder::new(&builder).build().unwrap_err();
    assert!(error.to_string().contains("URI is required"));
    HttpResponseBuilder::new(&builder).header("bad\nname", "value").build().unwrap_err();
}

#[cfg(feature = "json")]
struct FailedSerialization;

#[cfg(feature = "json")]
impl serde::Serialize for FailedSerialization {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("serialization-marker"))
    }
}

#[cfg(feature = "json")]
#[test]
fn gamma_survivor_json_errors_propagate_through_body_and_builders() {
    let builder = body_builder();
    assert!(
        builder
            .json(&FailedSerialization)
            .unwrap_err()
            .to_string()
            .contains("serialization-marker")
    );
    assert!(
        HttpRequestBuilder::new(&builder)
            .uri("https://example.test")
            .json(&FailedSerialization)
            .build()
            .unwrap_err()
            .to_string()
            .contains("serialization-marker")
    );
    assert!(
        HttpResponseBuilder::new(&builder)
            .json(&FailedSerialization)
            .build()
            .unwrap_err()
            .to_string()
            .contains("serialization-marker")
    );
    let body = builder.stream(
        futures::stream::iter([Err::<bytesbuf::BytesView, _>(HttpError::validation("json-stream-marker"))]),
        &HttpBodyOptions::default(),
    );
    assert!(
        block_on(body.into_json::<serde_json::Value>())
            .unwrap_err()
            .to_string()
            .contains("json-stream-marker")
    );
}

#[test]
fn gamma_survivor_stream_collection_error_is_not_a_panic() {
    let builder = body_builder();
    let stream = futures::stream::iter([Err::<bytesbuf::BytesView, _>(HttpError::validation("stream-marker"))]);
    let body = builder.stream(stream, &HttpBodyOptions::default());
    assert_eq!(format!("{body:?}"), "HttpBody { kind: Body }");
    assert!(block_on(body.into_text()).unwrap_err().to_string().contains("stream-marker"));
    let body = builder.stream(
        futures::stream::empty::<Result<bytesbuf::BytesView, HttpError>>(),
        &HttpBodyOptions::default(),
    );
    let error = bytesbuf::BytesView::try_from(body).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("body cannot be converted to byte sequence because it is not buffered")
    );
}

#[test]
fn gamma_survivor_router_diagnostics_and_original_are_preserved() {
    let error = Router::default()
        .resolve_uri(RouterContext::new(), "/path".parse().unwrap())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the target URI has no base URI and the routing did not produce one")
    );
    let router = Router::fixed(BaseUri::from_static("https://routed.test")).conflict_policy(BaseUriConflict::Fail);
    let error = router
        .resolve_uri(RouterContext::new(), "https://original.test/path".parse().unwrap())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("target URI already has a base URI; routing produced a conflicting base URI")
    );
    let builder = body_builder();
    let mut request = HttpRequestBuilder::new(&builder).uri("/path?x=1").build().unwrap();
    assert!(request.path_and_query().is_some());
    let router = Router::fixed(BaseUri::from_static("https://first.test")).conflict_policy(BaseUriConflict::UseRouted);
    router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();
    assert!(
        request.path_and_query().is_some(),
        "routing must preserve a built request's rendered path"
    );
    assert_eq!(request.uri(), "https://first.test/path?x=1");
    Router::fixed(BaseUri::from_static("https://second.test"))
        .conflict_policy(BaseUriConflict::UseRouted)
        .resolve_request_uri(RouterContext::new(), &mut request)
        .unwrap();
    assert!(request.path_and_query().is_some());
    assert_eq!(request.uri(), "https://second.test/path?x=1");
    let uris = request.extensions().get::<RequestUris>().unwrap();
    assert_eq!(uris.original().to_string().declassify_into(), "/path?x=1");
    assert_eq!(
        request.resolve_uri().unwrap().to_string().declassify_into(),
        "https://second.test/path?x=1"
    );
}

#[test]
fn gamma_survivor_one_byte_body_has_nonzero_content_length() {
    let builder = body_builder();
    let request = HttpRequestBuilder::new(&builder)
        .uri("https://example.test")
        .text("x")
        .build()
        .unwrap();
    assert_eq!(request.headers()["content-length"], "1");
    let response = HttpResponseBuilder::new(&builder).text("x").build().unwrap();
    assert_eq!(response.headers()["content-length"], "1");
}
