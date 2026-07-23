// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral tests for the HTTP `#[router]` boundary.

use std::borrow::Cow;
use std::sync::Arc;

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue, LOCATION};
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::{HeaderMap, Method, RawBody, Request, State, StatusCode, Uri, router};

#[derive(Clone)]
struct AppState {
    prefix: &'static str,
}

struct BooksApi {
    name: &'static str,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl BooksApi {
    #[route(GET, "/books")]
    #[route(HEAD, "/books")]
    async fn list_books(
        &self,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        state: State<Arc<AppState>>,
    ) -> (StatusCode, [(HeaderName, HeaderValue); 1], String) {
        let request_header = headers.get("x-request").and_then(|value| value.to_str().ok()).unwrap_or("missing");
        (
            StatusCode::OK,
            [(HeaderName::from_static("x-service"), HeaderValue::from_static("books"))],
            format!(
                "{}:{}:{}:{}:{}",
                self.name,
                state.prefix,
                method,
                uri.query().unwrap_or_default(),
                request_header
            ),
        )
    }

    #[route(GET, "/books/{id}")]
    async fn get_book(&self, id: u32) -> Result<Response<Body>, StatusCode> {
        if id == 42 {
            Ok(Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(LOCATION, "/books/42")
                .body(Body::from(format!("{}:{id}", self.name)))
                .expect("the generated response uses static valid metadata"))
        } else {
            Err(StatusCode::NOT_FOUND)
        }
    }

    #[route(GET, "/authors/{name}")]
    async fn get_author(&self, name: &str, method: Method) -> String {
        format!("{}:{method}:{name}", self.name)
    }

    #[route(GET, "/tags/{name}")]
    async fn get_tag(&self, name: Cow<'_, str>) -> Bytes {
        Bytes::from(format!("{}:{name}", self.name))
    }

    #[route(GET, "/reserved/{__routerama_request}")]
    async fn reserved_capture(&self, __routerama_request: &str) -> String {
        format!("{}:{__routerama_request}", self.name)
    }

    #[route(GET, "/empty")]
    async fn empty(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[tokio::test]
async fn extracts_request_parts_and_shared_state() {
    let api = BooksApi { name: "api" };
    let state = Arc::new(AppState { prefix: "state" });
    let request = Request::builder()
        .method(Method::HEAD)
        .uri("/books?sort=title")
        .header("x-request", "present")
        .body(())
        .expect("the test request uses valid static metadata");

    let response = api.route(request, &state).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-service"], "books");
    assert_eq!(body_bytes(response.into_body()).await, b"api:state:HEAD:sort=title:present"[..]);
}

#[tokio::test]
async fn preserves_direct_captures_and_converts_heterogeneous_responses() {
    let api = BooksApi { name: "api" };
    let state = Arc::new(AppState { prefix: "state" });

    let response = api.route(request(Method::GET, "/books/42", ()), &state).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()[LOCATION], "/books/42");
    assert_eq!(body_bytes(response.into_body()).await, b"api:42"[..]);

    let response = api.route(request(Method::GET, "/books/7", ()), &state).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(body_bytes(response.into_body()).await.is_empty());

    let response = api.route(request(Method::GET, "/authors/ursula", ()), &state).await;
    assert_eq!(body_bytes(response.into_body()).await, b"api:GET:ursula"[..]);

    let response = api.route(request(Method::GET, "/tags/science%20fiction", ()), &state).await;
    assert_eq!(body_bytes(response.into_body()).await, b"api:science fiction"[..]);

    let response = api.route(request(Method::GET, "/reserved/value", ()), &state).await;
    assert_eq!(body_bytes(response.into_body()).await, b"api:value"[..]);

    let response = api.route(request(Method::GET, "/empty", ()), &state).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn routing_misses_and_capture_failures_are_http_responses() {
    let api = BooksApi { name: "api" };
    let state = Arc::new(AppState { prefix: "state" });

    let missing = api.route(request(Method::GET, "/missing", ()), &state).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let invalid = api.route(request(Method::GET, "/books/not-a-number", ()), &state).await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

struct BodyApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl BodyApi {
    #[route(POST, "/echo/{id}")]
    async fn echo(
        &self,
        method: Method,
        #[body] body: RawBody<Vec<u8>>,
        id: u32,
        headers: HeaderMap,
    ) -> ([(HeaderName, HeaderValue); 1], Vec<u8>) {
        assert_eq!(method, Method::POST);
        assert_eq!(headers["x-body"], "owned");
        let mut body = body.into_inner();
        body.extend_from_slice(format!(":{id}").as_bytes());
        ([(HeaderName::from_static("x-body-seen"), HeaderValue::from_static("yes"))], body)
    }
}

#[tokio::test]
async fn an_explicit_body_parameter_owns_the_raw_body_in_any_position() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/echo/17")
        .header("x-body", "owned")
        .body(b"payload".to_vec())
        .expect("the test request uses valid static metadata");

    let response = BodyApi.route(request, &()).await;

    assert_eq!(response.headers()["x-body-seen"], "yes");
    assert_eq!(body_bytes(response.into_body()).await, b"payload:17"[..]);
}

struct PluginsApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PluginsApi {
    #[route(GET, "/health")]
    async fn health(&self) -> &'static str {
        "static"
    }

    #[route(dynamic)]
    async fn plugin(&self, method: Method, #[capture] name: String) -> String {
        format!("{method}:{name}")
    }

    #[route(dynamic)]
    async fn item(&self, #[capture] id: u32) -> StatusCode {
        if id == 42 { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
    }
}

#[tokio::test]
async fn configured_dynamic_routes_use_the_same_http_contract() {
    let router = PluginsApi::router_builder()
        .add_plugin("GET", "/plugins/{name}")
        .add_item("GET", "/items/{id}")
        .build()
        .expect("dynamic routes are valid");
    let api = PluginsApi;

    let response = router.route(&api, request(Method::GET, "/health", ()), &()).await;
    assert_eq!(body_bytes(response.into_body()).await, b"static"[..]);

    let response = router.route(&api, request(Method::GET, "/plugins/tracing", ()), &()).await;
    assert_eq!(body_bytes(response.into_body()).await, b"GET:tracing"[..]);

    let response = router.route(&api, request(Method::GET, "/items/42", ()), &()).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = router.route(&api, request(Method::GET, "/items/not-a-number", ()), &()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn dynamic_service_builder_reports_configuration_errors() {
    let error = PluginsApi::router_builder()
        .add_plugin("GET", "/plugins/{wrong}")
        .build()
        .expect_err("plugin captures are wrong and item was not registered");
    let message = error.to_string();
    assert!(message.contains("do not match"), "{message}");
    assert!(message.contains("add_item"), "{message}");
}

fn request<B>(method: Method, uri: &str, body: B) -> Request<B> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .expect("the test request uses valid static metadata")
}

async fn body_bytes<B>(body: B) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: std::fmt::Debug,
{
    body.collect().await.expect("the response body is infallible").to_bytes()
}

#[test]
fn prototype_body_implements_http_body_without_boxing() {
    fn assert_http_body<T: http_body::Body<Data = Bytes>>() {}

    assert_http_body::<Body>();
}
