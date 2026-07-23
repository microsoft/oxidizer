// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs a generated static service with an explicitly erased mounted fallback.

use http::StatusCode;
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::mount::{ErasedMountRouter, ErasedMountService, MountedRequest, MountedService};
use routerama::route::{Request, router};

#[derive(Clone)]
struct AppState {
    deployment: &'static str,
}

struct App;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl App {
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

struct PluginService;

impl MountedService<Body, AppState> for PluginService {
    type Response = Response<Body>;

    async fn call<'a>(&'a self, request: MountedRequest<'a, Body>, state: &'a AppState) -> Self::Response
    where
        Body: 'a,
    {
        let name = request.decoded_capture("name").expect("the template captures `name`");
        core::future::ready(()).await;
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .header("x-plugin", name.as_ref())
            .body(Body::from(format!("{}:{name}", state.deployment)))
            .expect("plugin metadata is valid")
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let plugin = ErasedMountService::new(PluginService);
    let mounts = ErasedMountRouter::builder()
        .mount("GET", "/plugins/{name}", plugin.clone())
        .mount("GET", "/extensions/{name}", plugin)
        .build()
        .expect("mounted aliases are valid");
    let state = AppState { deployment: "west" };

    let static_response = App
        .route_with_erased_mounts(Request::get("/health").body(Body::empty()).expect("valid request"), &state, &mounts)
        .await;
    assert_eq!(static_response.status(), StatusCode::NO_CONTENT);

    let mounted_response = App
        .route_with_erased_mounts(
            Request::get("/plugins/search").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(mounted_response.status(), StatusCode::ACCEPTED);
    assert_eq!(mounted_response.headers()["x-plugin"], "search");
    assert_eq!(
        mounted_response
            .into_body()
            .collect()
            .await
            .expect("mounted response body succeeds")
            .to_bytes(),
        b"west:search"[..]
    );
}
