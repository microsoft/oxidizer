// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! URI assembly errors must preserve both diagnostics and request state.

use http_extensions::routing::{BaseUriConflict, RequestUris, Router, RouterContext};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequestBuilder};
use ohno::Labeled as _;
use templated_uri::{BaseUri, PathAndQuery, Uri};

fn bodies() -> HttpBodyBuilder {
    HttpBodyBuilder::new(
        bytesbuf::mem::OpaqueMemory::new(bytesbuf::mem::GlobalPool::new()),
        &tick::Clock::new_frozen(),
    )
}

fn long_base() -> Result<BaseUri, templated_uri::UriError> {
    format!("https://example.test/{}/", "a".repeat(40_000)).parse()
}

fn long_path() -> String {
    format!("/{}", "b".repeat(40_000))
}

struct RejectedUri;

impl TryFrom<RejectedUri> for Uri {
    type Error = HttpError;

    fn try_from(_: RejectedUri) -> Result<Self, Self::Error> {
        Err(HttpError::validation("controlled URI conversion failure"))
    }
}

#[test]
fn request_builder_returns_missing_uri_and_join_overflow_errors() {
    let bodies = bodies();
    let error = HttpRequestBuilder::new(&bodies).build().unwrap_err();
    assert_eq!(error.label(), "uri_missing");
    assert!(error.to_string().contains("URI is required when building the request"));
    let error = HttpRequestBuilder::new(&bodies).uri(RejectedUri).build().unwrap_err();
    assert_eq!(error.label(), "validation");
    assert!(error.to_string().contains("controlled URI conversion failure"));

    let uri = Uri::from_parts(
        long_base().unwrap(),
        PathAndQuery::from(long_path().parse::<http::uri::PathAndQuery>().unwrap()),
    );
    let error = HttpRequestBuilder::new(&bodies).uri(uri).build().unwrap_err();
    assert_eq!(error.label(), "uri_invalid");
}

#[test]
fn routing_join_overflow_leaves_built_and_handmade_requests_unchanged() {
    let bodies = bodies();
    let path = long_path();
    let router = Router::fixed(long_base().unwrap()).conflict_policy(BaseUriConflict::UseRouted);
    for built in [false, true] {
        let mut request = if built {
            HttpRequestBuilder::new(&bodies).uri(path.as_str()).build().unwrap()
        } else {
            http::Request::builder().uri(&path).body(bodies.empty()).unwrap()
        };
        request
            .headers_mut()
            .insert("x-marker", http::HeaderValue::from_static("unchanged"));
        let before = request.uri().clone();
        let error = router.resolve_request_uri(RouterContext::new(), &mut request).unwrap_err();
        assert_eq!(error.label(), "uri_invalid");
        assert_eq!(request.uri(), &before);
        assert_eq!(request.headers()["x-marker"], "unchanged");
        let uris = request.extensions().get::<RequestUris>();
        assert_eq!(uris.is_some(), built);
        if let Some(uris) = uris {
            assert!(uris.routed().is_none());
            assert_eq!(http::Uri::try_from(uris.original().clone()).unwrap(), before);
        }
    }
}
