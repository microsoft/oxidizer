// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Round-two request and routing failure contracts.

use http_extensions::routing::{BaseUriConflict, Router, RouterContext};
use http_extensions::{HttpBodyBuilder, HttpRequestBuilder, RequestExt};
use ohno::{ErrorExt, Labeled};
use templated_uri::{BaseUri, Uri, templated};

#[templated(template = "/{+raw}", unredacted)]
struct RawPath {
    raw: String,
}

fn bodies() -> HttpBodyBuilder {
    HttpBodyBuilder::new(
        bytesbuf::mem::OpaqueMemory::new(bytesbuf::mem::GlobalPool::new()),
        &tick::Clock::new_frozen(),
    )
}

#[test]
fn gamma_round2_missing_uri_is_a_validation_error() {
    let error = HttpRequestBuilder::new(&bodies()).build().unwrap_err();
    assert_eq!(error.label(), "uri_missing");
    assert_eq!(error.message(), "URI is required when building the request");
}

#[test]
fn gamma_round2_invalid_template_render_is_returned_not_panicked() {
    let error = HttpRequestBuilder::new(&bodies())
        .uri(Uri::from(RawPath { raw: "\n".to_owned() }))
        .build()
        .unwrap_err();
    assert_eq!(error.label(), "uri_invalid");
}

#[test]
fn gamma_round2_routing_built_path_is_preserved_across_retries() {
    let mut request = HttpRequestBuilder::new(&bodies()).uri("/original?x=1").build().unwrap();
    let router = Router::fixed(BaseUri::from_static("https://example.test/api/")).conflict_policy(BaseUriConflict::UseRouted);
    for _ in 0..2 {
        router.resolve_request_uri(RouterContext::new(), &mut request).unwrap();
        assert_eq!(request.path_and_query().unwrap().template(), "/original?x=1");
        assert_eq!(request.uri(), "https://example.test/api/original?x=1");
    }
}
