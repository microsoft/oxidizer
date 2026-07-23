// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral tests for `#[service]`.

use std::borrow::Cow;

use routerama::{ResolveError, service};

struct RequestContext {
    prefix: &'static str,
}

struct BooksApi {
    name: &'static str,
}

#[service]
impl BooksApi {
    #[route(GET, "/books")]
    #[route(HEAD, "/books")]
    async fn list_books(&self, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:all", self.name, request.prefix)).await
    }

    #[route(GET, "/books/{id}")]
    async fn get_book(&self, id: u32, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:{id}", self.name, request.prefix)).await
    }

    #[route(GET, "/authors/{name}")]
    async fn get_author(&self, request: &RequestContext, name: &str) -> String {
        std::future::ready(format!("{}:{}:{name}", self.name, request.prefix)).await
    }

    #[route(GET, "/tags/{name}")]
    async fn get_tag(&self, name: Cow<'_, str>, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:{name}", self.name, request.prefix)).await
    }

    #[route(GET, "/reserved/{__routerama_request_context}")]
    async fn reserved_capture(&self, __routerama_request_context: &str, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:{__routerama_request_context}", self.name, request.prefix)).await
    }

    fn service_name(&self) -> &'static str {
        self.name
    }
}

#[tokio::test]
async fn dispatches_static_capture_and_borrowing_handlers() {
    let api = BooksApi { name: "books" };
    let request = RequestContext { prefix: "response" };

    assert_eq!(api.dispatch("GET", "/books", &request).await, Ok("books:response:all".into()));
    assert_eq!(api.dispatch("HEAD", "/books", &request).await, Ok("books:response:all".into()));
    assert_eq!(api.dispatch("GET", "/books/42", &request).await, Ok("books:response:42".into()));
    assert_eq!(
        api.dispatch("GET", "/authors/ursula", &request).await,
        Ok("books:response:ursula".into())
    );
    assert_eq!(
        api.dispatch("GET", "/tags/science%20fiction", &request).await,
        Ok("books:response:science fiction".into())
    );
    assert_eq!(
        api.dispatch("GET", "/reserved/value", &request).await,
        Ok("books:response:value".into())
    );
    assert_eq!(api.service_name(), "books");
}

#[tokio::test]
async fn dispatch_preserves_resolution_errors() {
    let api = BooksApi { name: "books" };
    let request = RequestContext { prefix: "response" };

    assert_eq!(
        api.dispatch("GET", "/missing", &request).await,
        Err(ResolveError::NotFound("/missing"))
    );
    assert_eq!(
        api.dispatch("GET", "/books/not-a-number", &request).await,
        Err(ResolveError::InvalidCapture("id"))
    );
}

struct PluginsApi {
    prefix: &'static str,
}

#[service]
impl PluginsApi {
    #[route(GET, "/health")]
    async fn health(&self, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:healthy", self.prefix, request.prefix)).await
    }

    #[route(dynamic)]
    async fn plugin(&self, name: String, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:{name}", self.prefix, request.prefix)).await
    }

    #[route(dynamic)]
    async fn item(&self, request: &RequestContext, id: u32) -> String {
        std::future::ready(format!("{}:{}:{id}", self.prefix, request.prefix)).await
    }

    #[route(dynamic)]
    async fn dynamic_health(&self, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{}:dynamic", self.prefix, request.prefix)).await
    }
}

#[tokio::test]
async fn configured_router_dispatches_static_and_dynamic_handlers() {
    let api = PluginsApi { prefix: "plugins" };
    let request = RequestContext { prefix: "response" };
    let router = PluginsApi::router_builder()
        .add_plugin(routerama::HttpMethod::GET, "/plugins/{name}")
        .add_plugin(routerama::HttpMethod::POST, "/extensions/{name}")
        .add_item(routerama::HttpMethod::GET, "/items/{id}")
        .add_dynamic_health(routerama::HttpMethod::GET, "/health")
        .build()
        .expect("dynamic routes are valid");

    assert_eq!(
        router.dispatch(&api, "GET", "/health", &request).await,
        Ok("plugins:response:healthy".into())
    );
    assert_eq!(
        router.dispatch(&api, "GET", "/plugins/tracing", &request).await,
        Ok("plugins:response:tracing".into())
    );
    assert_eq!(
        router.dispatch(&api, "POST", "/extensions/cache", &request).await,
        Ok("plugins:response:cache".into())
    );
    assert_eq!(
        router.dispatch(&api, "GET", "/items/42", &request).await,
        Ok("plugins:response:42".into())
    );
    assert_eq!(
        router.dispatch(&api, "GET", "/items/not-a-number", &request).await,
        Err(ResolveError::InvalidCapture("id"))
    );
}

#[test]
fn dynamic_service_builder_reports_all_configuration_errors() {
    let error = PluginsApi::router_builder()
        .add_plugin(routerama::HttpMethod::GET, "/plugins/{wrong}")
        .build()
        .expect_err("plugin captures are wrong and item was not registered");
    let message = error.to_string();
    assert!(message.contains("do not match"), "{message}");
    assert!(message.contains("add_item"), "{message}");
}

struct DynamicOnlyApi;

#[service]
impl DynamicOnlyApi {
    #[route(dynamic)]
    async fn echo(&self, value: String, request: &RequestContext) -> String {
        std::future::ready(format!("{}:{value}", request.prefix)).await
    }
}

#[tokio::test]
async fn dynamic_only_service_dispatches_through_its_router() {
    let api = DynamicOnlyApi;
    let request = RequestContext { prefix: "response" };
    let router = DynamicOnlyApi::router_builder()
        .add_echo(routerama::HttpMethod::GET, "/echo/{value}")
        .build()
        .expect("dynamic route is valid");

    assert_eq!(
        router.dispatch(&api, "GET", "/echo/hello", &request).await,
        Ok("response:hello".into())
    );
}

struct OwnedContext {
    label: String,
}

struct OwnedContextApi;

#[service(context)]
impl OwnedContextApi {
    #[route(GET, "/owned")]
    async fn owned(&self, context: OwnedContext) -> String {
        std::future::ready(context.label).await
    }
}

#[tokio::test]
async fn context_mode_can_forward_an_owned_value() {
    let api = OwnedContextApi;
    let context = OwnedContext { label: "owned".into() };

    assert_eq!(api.dispatch("GET", "/owned", context).await, Ok("owned".into()));
}

struct MutableContext {
    calls: usize,
}

struct MutableContextApi;

#[service(context)]
impl MutableContextApi {
    #[route(GET, "/static/{id}")]
    async fn static_route(&self, context: &mut MutableContext, id: u32) -> String {
        context.calls += 1;
        std::future::ready(format!("static:{id}")).await
    }

    #[route(dynamic)]
    async fn dynamic_route(&self, context: &mut MutableContext, name: String) -> String {
        context.calls += 1;
        std::future::ready(format!("dynamic:{name}")).await
    }
}

#[tokio::test]
async fn context_mode_can_forward_one_mutable_context_to_mixed_routes() {
    let api = MutableContextApi;
    let mut context = MutableContext { calls: 0 };
    let router = MutableContextApi::router_builder()
        .add_dynamic_route(routerama::HttpMethod::GET, "/dynamic/{name}")
        .build()
        .expect("dynamic route is valid");

    assert_eq!(
        router.dispatch(&api, "GET", "/static/42", &mut context).await,
        Ok("static:42".into())
    );
    assert_eq!(
        router.dispatch(&api, "GET", "/dynamic/plugin", &mut context).await,
        Ok("dynamic:plugin".into())
    );
    assert_eq!(context.calls, 2);
}
