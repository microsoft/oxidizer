// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Intentional overlap, typed routing fallback, and extractor catcher.

use http_body_util::BodyExt as _;
use routerama::response::{Body, IntoResponse, Response};
use routerama::route::{FromRequestParts, Request, RequestParts, RouteFailure, StatusCode, router};

#[derive(Clone, Copy, Debug)]
struct MissingTrace;

impl IntoResponse for MissingTrace {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        StatusCode::BAD_REQUEST.into_response()
    }
}

struct RequiredTrace;

impl<S: ?Sized> FromRequestParts<'_, S> for RequiredTrace {
    type Rejection = MissingTrace;

    fn from_request_parts(parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.headers.contains_key("x-trace").then_some(Self).ok_or(MissingTrace)
    }
}

struct Reports;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Reports {
    #[route(GET, "/reports/{id}", produces = "application/json", priority = 10)]
    async fn json(&self, id: u32) -> String {
        format!(r#"{{"id":{id}}}"#)
    }

    #[route(GET, "/reports/{id}", produces = "text/plain", priority = 0)]
    async fn text(&self, id: u32) -> String {
        format!("report {id}")
    }

    #[route(GET, "/secure")]
    async fn secure(&self, trace: RequiredTrace) -> StatusCode {
        let _ = trace;
        StatusCode::NO_CONTENT
    }

    #[catch(MissingTrace, from = RequiredTrace)]
    async fn catch_trace(&self, _rejection: MissingTrace) -> (StatusCode, &'static str) {
        (StatusCode::UNAUTHORIZED, "trace required")
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> (StatusCode, String) {
        (failure.status(), failure.to_string())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let request = Request::get("/reports/42")
        .header("accept", "application/json")
        .body(())
        .expect("static request metadata is valid");
    let response = Reports.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(bytes(response).await, br#"{"id":42}"#[..]);

    let caught = Reports.route(Request::get("/secure").body(()).expect("valid request"), &()).await;
    assert_eq!(caught.status(), StatusCode::UNAUTHORIZED);

    let missing = Reports.route(Request::get("/missing").body(()).expect("valid request"), &()).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

async fn bytes<B>(response: Response<B>) -> bytes::Bytes
where
    B: http_body::Body<Data = bytes::Bytes>,
    B::Error: core::fmt::Debug,
{
    response
        .into_body()
        .collect()
        .await
        .expect("example response body succeeds")
        .to_bytes()
}
