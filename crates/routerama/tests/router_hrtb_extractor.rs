// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Higher-ranked extractor types.
//!
//! A `for<'a>` binder inside an extractor type binds its own lifetimes, so the
//! generated request-parts contract must neither rewrite nor reject them.

use bytes::Bytes;
use http_body_util::BodyExt as _;
use routerama::response::Response;
use routerama::route::{FromRequestParts, Request, RequestParts, StatusCode, router};

trait Tagged<'a> {
    fn tag(&self) -> &'a str;
}

struct StaticTag;

impl<'a> Tagged<'a> for StaticTag {
    fn tag(&self) -> &'a str {
        "static"
    }
}

struct BoxedTag(Box<dyn for<'a> Tagged<'a> + Send>);

impl<'request, S: ?Sized> FromRequestParts<'request, S> for BoxedTag {
    type Rejection = StatusCode;

    fn from_request_parts(parts: &'request RequestParts, state: &S) -> Result<Self, Self::Rejection> {
        let _ = (parts, state);
        Ok(Self(Box::new(StaticTag)))
    }
}

struct Api;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Api {
    #[route(GET, "/tag")]
    async fn tag(&self, tag: BoxedTag) -> String {
        tag.0.tag().to_owned()
    }
}

async fn body<B>(response: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: core::fmt::Debug,
{
    response.into_body().collect().await.expect("body succeeds").to_bytes()
}

#[tokio::test(flavor = "current_thread")]
async fn a_higher_ranked_extractor_type_is_accepted() {
    let response = Api.route(Request::get("/tag").body(()).expect("valid request"), &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"static"[..]);
}
