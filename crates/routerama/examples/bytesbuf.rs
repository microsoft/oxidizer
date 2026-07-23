// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fragmented responses and prepared templates backed by application memory.

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use http_body_util::BodyExt as _;
use routerama::response::bytesbuf::template::{BytesViewTemplate, json_number, json_string};
use routerama::route::{Request, State, router};

#[derive(Clone, Debug)]
struct AppState {
    memory: GlobalPool,
    document: BytesViewTemplate<3>,
}

struct Api;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
)]
#[router(state = AppState, heterogeneous_data)]
impl Api {
    #[route(GET, "/document/{id}")]
    async fn document(&self, id: u32, state: State<AppState>) -> BytesView {
        state.document.render(&state.memory, (json_number(id), json_string("routerama")))
    }

    #[route(GET, "/health")]
    async fn health(&self) -> &'static str {
        "healthy"
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let memory = GlobalPool::new();
    let state = AppState {
        document: BytesViewTemplate::prepare(&memory, [br#"{"id":"#, br#","name":"#, b"}"]),
        memory,
    };
    let request = Request::get("/document/42").body(()).expect("valid request");
    let response = Api.route(request, &state).await;
    let body = response.into_body().collect().await.expect("the response body succeeds").to_bytes();

    assert_eq!(body, br#"{"id":42,"name":"routerama"}"#[..]);
}
