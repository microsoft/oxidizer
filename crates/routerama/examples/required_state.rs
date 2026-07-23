// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Definition-time validation for a service with explicit shared state.

use std::sync::Arc;

use http_body_util::BodyExt as _;
use routerama::response::Response;
use routerama::route::{FromRef, FromRequestParts, Request, RequestParts, State, StatusCode, router};

#[derive(Clone)]
struct AppState {
    application: Arc<str>,
    revision: u32,
}

#[derive(Clone)]
struct ApplicationName(Arc<str>);

impl FromRef<AppState> for ApplicationName {
    fn from_ref(input: &AppState) -> Self {
        Self(Arc::clone(&input.application))
    }
}

#[derive(Clone, Copy)]
struct Revision(u32);

impl FromRef<AppState> for Revision {
    fn from_ref(input: &AppState) -> Self {
        Self(input.revision)
    }
}

struct RequiredHeader<'request>(&'request str);

impl<'request> FromRequestParts<'request, AppState> for RequiredHeader<'request> {
    type Rejection = StatusCode;

    fn from_request_parts(parts: &'request RequestParts, state: &AppState) -> Result<Self, Self::Rejection> {
        let _ = state;
        parts
            .headers
            .get("x-request")
            .and_then(|value| value.to_str().ok())
            .map(Self)
            .ok_or(StatusCode::BAD_REQUEST)
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
#[router(state = AppState)]
impl Api {
    #[route(GET, "/")]
    async fn home(&self, application: State<ApplicationName>, revision: State<Revision>, request: RequiredHeader<'_>) -> String {
        format!("{}:{}:{}", application.0.0, revision.0.0, request.0)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let state = AppState {
        application: Arc::from("routerama"),
        revision: 3,
    };
    let request = Request::get("/")
        .header("x-request", "example")
        .body(())
        .expect("static request metadata is valid");
    let response = Api.route(request, &state).await;
    assert_eq!(bytes(response).await, b"routerama:3:example"[..]);
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
