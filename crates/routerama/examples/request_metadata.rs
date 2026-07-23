// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrowed and owned request metadata plus typed extensions.

use http::header::ACCEPT_LANGUAGE;
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::{
    ClonedExtension, ExtensionRef, Extensions, FromRequestParts, HeaderMap, Method, Request, RequestParts, StatusCode, Uri, Version, router,
};

/// A typed value an outer transport layer inserts into request extensions.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestId(String);

/// A tenant identifier only the routing layer knows about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TenantId(u32);

/// A custom zero-copy extractor over one request header.
struct AcceptLanguage<'request>(&'request str);

impl<'request, S: ?Sized> FromRequestParts<'request, S> for AcceptLanguage<'request> {
    type Rejection = StatusCode;

    fn from_request_parts(parts: &'request RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok())
            .map(Self)
            .ok_or(StatusCode::BAD_REQUEST)
    }
}

struct Inspector;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Inspector {
    /// Borrows the request head without cloning any of it.
    #[route(GET, "/borrowed/{topic}")]
    async fn borrowed(&self, topic: &str, method: &Method, uri: &Uri, headers: &HeaderMap, language: AcceptLanguage<'_>) -> String {
        // The borrowed capture and the borrowed header slice both point
        // straight at the request head.
        assert!(core::ptr::eq(topic.as_ptr(), uri.path()["/borrowed/".len()..].as_ptr()));
        assert!(core::ptr::eq(language.0.as_ptr(), headers["accept-language"].as_bytes().as_ptr()));

        // References survive an await point: generated dispatch owns the
        // request parts until the handler completes.
        core::future::ready(()).await;

        format!("{method} {} topic={topic} lang={}", uri.path(), language.0)
    }

    /// Takes owned metadata: every clone or copy is explicit in the signature.
    #[route(GET, "/owned")]
    async fn owned(&self, method: Method, uri: Uri, version: Version, headers: HeaderMap, parts: &RequestParts) -> String {
        assert_eq!(parts.method, method);
        assert_eq!(version, Version::HTTP_11);
        format!(
            "{method} {} accept={}",
            uri.path(),
            headers["accept"].to_str().expect("the example sends ASCII headers")
        )
    }

    /// Reads typed extensions three ways: borrowed map, borrowed value, clone.
    #[route(GET, "/extensions")]
    async fn extensions(
        &self,
        extensions: &Extensions,
        request_id: ExtensionRef<'_, RequestId>,
        tenant: ClonedExtension<TenantId>,
    ) -> String {
        // `ExtensionRef` performs one type-map lookup and hands back `&T`.
        assert!(core::ptr::eq(
            request_id.get(),
            extensions.get::<RequestId>().expect("the request carries an id")
        ));
        format!("id={} tenant={}", request_id.get().0, tenant.into_inner().0)
    }

    /// Requires an extension the caller may have forgotten to insert.
    #[route(GET, "/requires-tenant")]
    async fn requires_tenant(&self, tenant: ExtensionRef<'_, TenantId>) -> String {
        tenant.get().0.to_string()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let request = Request::get("/borrowed/routing")
        .header(ACCEPT_LANGUAGE, "en-GB")
        .body(Body::empty())
        .expect("static request metadata is valid");
    let response = Inspector.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"GET /borrowed/routing topic=routing lang=en-GB"[..]);

    let request = Request::get("/owned")
        .header("accept", "text/plain")
        .body(Body::empty())
        .expect("static request metadata is valid");
    let response = Inspector.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"GET /owned accept=text/plain"[..]);

    let mut request = Request::get("/extensions")
        .body(Body::empty())
        .expect("static request metadata is valid");
    _ = request.extensions_mut().insert(RequestId(String::from("req-7")));
    _ = request.extensions_mut().insert(TenantId(42));
    let response = Inspector.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"id=req-7 tenant=42"[..]);

    // A missing typed extension is a server configuration failure, not a
    // client error, so the extractor rejects with 500 and the handler never
    // runs.
    let response = Inspector
        .route(
            Request::get("/requires-tenant")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // A custom extractor rejection short-circuits the same way.
    let response = Inspector
        .route(
            Request::get("/borrowed/routing")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

async fn body<B>(response: Response<B>) -> bytes::Bytes
where
    B: http_body::Body<Data = bytes::Bytes>,
    B::Error: core::fmt::Debug,
{
    response
        .into_body()
        .collect()
        .await
        .expect("example response bodies succeed")
        .to_bytes()
}
