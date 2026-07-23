// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded HTML form extraction without Serde.
//!
//! Run with `cargo run -p routerama --example form --features form`.

use http_body_util::BodyExt as _;
use routerama::query::FromQuery;
use routerama::response::Body;
use routerama::route::form::Form;
use routerama::route::{Request, router};

#[derive(FromQuery)]
struct Registration {
    name: String,
    newsletter: Option<bool>,
    topic: Vec<String>,
}

struct Registrations;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Registrations {
    #[route(POST, "/registrations")]
    async fn register(&self, #[body] form: Form<Registration, 1024>) -> String {
        let registration = form.into_inner();
        format!(
            "{}:{}:{}",
            registration.name,
            registration.newsletter.unwrap_or(false),
            registration.topic.join(",")
        )
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let request = Request::post("/registrations")
        .header("content-type", "application/x-www-form-urlencoded; charset=utf-8")
        .body(Body::from("name=Ada+Lovelace&newsletter=true&topic=rust&topic=web"))
        .expect("static request metadata is valid");

    let response = Registrations.route(request, &()).await;
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes();

    assert_eq!(body, b"Ada Lovelace:true:rust,web"[..]);
}
