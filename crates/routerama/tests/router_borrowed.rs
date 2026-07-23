// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for lifetime-aware request-parts extraction.

use std::cell::Cell;
use std::fmt;
#[cfg(not(miri))]
use std::future::Future as _;
#[cfg(not(miri))]
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(miri))]
use std::task::{Context, Poll, Waker};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use http::header::USER_AGENT;
use routerama::response::{Body, IntoResponse, Response};
use routerama::route::{
    ClonedExtension, ExtensionRef, Extensions, FromRequestParts, HeaderMap, Method, MissingExtension, RawBody, Request, RequestParts,
    StatusCode, Uri, Version, router,
};

#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn total_bytes_allocated(session: &Session, operation_name: &str) -> u64 {
    let missing_operation = format!("operation {operation_name:?} must match a name registered with Session::operation on this session");

    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == operation_name).then(|| operation.total_bytes_allocated()))
        .expect(&missing_operation)
}

#[derive(Debug)]
struct TrackedExtension {
    label: &'static str,
    clones: Arc<AtomicUsize>,
}

impl Clone for TrackedExtension {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            label: self.label,
            clones: Arc::clone(&self.clones),
        }
    }
}

struct MetadataApi {
    header_bytes: usize,
    uri_bytes: usize,
    extension: usize,
    calls: AtomicUsize,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::too_many_arguments,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "the exhaustive metadata handler and generated router require this signature; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl MetadataApi {
    #[route(GET, "/metadata")]
    async fn metadata(
        &self,
        method: Method,
        method_ref: &Method,
        uri: &Uri,
        owned_uri: Uri,
        version: Version,
        version_ref: &Version,
        headers: &HeaderMap,
        owned_headers: HeaderMap,
        extensions: &Extensions,
        parts: &RequestParts,
        extension: ExtensionRef<'_, TrackedExtension>,
        cloned_extension: ClonedExtension<TrackedExtension>,
    ) -> StatusCode {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(method, Method::GET);
        assert_eq!(method_ref, &method);
        assert!(std::ptr::eq(std::ptr::from_ref(method_ref), std::ptr::from_ref(&parts.method)));
        assert!(std::ptr::eq(std::ptr::from_ref(uri), std::ptr::from_ref(&parts.uri)));
        assert!(std::ptr::eq(std::ptr::from_ref(version_ref), std::ptr::from_ref(&parts.version)));
        assert!(std::ptr::eq(std::ptr::from_ref(headers), std::ptr::from_ref(&parts.headers)));
        assert!(std::ptr::eq(std::ptr::from_ref(extensions), std::ptr::from_ref(&parts.extensions)));
        assert_eq!(uri.path().as_ptr().addr(), self.uri_bytes);
        assert_eq!(headers["x-request"].as_bytes().as_ptr().addr(), self.header_bytes);
        assert_eq!(std::ptr::from_ref(extension.get()).addr(), self.extension);
        assert_eq!(
            extensions.get::<TrackedExtension>().map(|value| std::ptr::from_ref(value).addr()),
            Some(self.extension)
        );
        assert_eq!(version, Version::HTTP_2);
        assert_eq!(*version_ref, version);
        assert_eq!(owned_uri, *uri);
        assert_eq!(owned_headers, *headers);
        assert_eq!(extension.label, "request");
        assert_eq!(cloned_extension.label, "request");

        tokio::task::yield_now().await;

        assert_eq!(headers["x-request"], "present");
        assert_eq!(uri.path(), "/metadata");
        assert_eq!(extension.label, "request");
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/borrowed-extension")]
    async fn borrowed_extension(&self, extension: ExtensionRef<'_, TrackedExtension>) -> StatusCode {
        assert_eq!(extension.label, "request");
        StatusCode::NO_CONTENT
    }

    #[route(POST, "/body")]
    async fn body(&self, headers: &HeaderMap, #[body] body: RawBody<Vec<u8>>, uri: &Uri, parts: &RequestParts) -> Vec<u8> {
        assert!(std::ptr::eq(std::ptr::from_ref(headers), std::ptr::from_ref(&parts.headers)));
        assert!(std::ptr::eq(std::ptr::from_ref(uri), std::ptr::from_ref(&parts.uri)));
        tokio::task::yield_now().await;
        assert_eq!(headers["x-request"], "present");
        assert_eq!(uri.path(), "/body");
        body.into_inner()
    }
}

#[tokio::test]
async fn borrowed_metadata_preserves_identity_and_only_explicit_ownership_clones() {
    let clone_count = Arc::new(AtomicUsize::new(0));
    let mut request = Request::builder()
        .method(Method::GET)
        .uri("/metadata?sort=title")
        .version(Version::HTTP_2)
        .header("x-request", "present")
        .body(Vec::new())
        .expect("the test request uses valid static metadata");
    request.extensions_mut().insert(TrackedExtension {
        label: "request",
        clones: Arc::clone(&clone_count),
    });
    let api = MetadataApi {
        header_bytes: request.headers()["x-request"].as_bytes().as_ptr().addr(),
        uri_bytes: request.uri().path().as_ptr().addr(),
        extension: request
            .extensions()
            .get::<TrackedExtension>()
            .map(|value| std::ptr::from_ref(value).addr())
            .expect("the extension was inserted"),
        calls: AtomicUsize::new(0),
    };

    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(api.calls.load(Ordering::SeqCst), 1);
    assert_eq!(clone_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn borrowed_typed_extensions_do_not_clone() {
    let clone_count = Arc::new(AtomicUsize::new(0));
    let mut request = Request::get("/borrowed-extension")
        .body(Vec::new())
        .expect("the test request uses valid static metadata");
    request.extensions_mut().insert(TrackedExtension {
        label: "request",
        clones: Arc::clone(&clone_count),
    });
    let extension = request
        .extensions()
        .get::<TrackedExtension>()
        .map(|value| std::ptr::from_ref(value).addr())
        .expect("the extension was inserted");
    let api = MetadataApi {
        header_bytes: 0,
        uri_bytes: 0,
        extension,
        calls: AtomicUsize::new(0),
    };

    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(clone_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn borrowed_metadata_remains_live_across_body_extraction_and_handler_await() {
    let request = Request::post("/body")
        .header("x-request", "present")
        .body(b"body".to_vec())
        .expect("the test request uses valid static metadata");
    let api = MetadataApi {
        header_bytes: 0,
        uri_bytes: 0,
        extension: 0,
        calls: AtomicUsize::new(0),
    };

    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn missing_typed_extensions_are_typed_server_errors() {
    let request = Request::get("/borrowed-extension")
        .body(Vec::new())
        .expect("the test request uses valid static metadata");
    let api = MetadataApi {
        header_bytes: 0,
        uri_bytes: 0,
        extension: 0,
        calls: AtomicUsize::new(0),
    };

    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);

    let rejection = MissingExtension::<TrackedExtension>::new();
    assert!(rejection.to_string().contains("TrackedExtension"));
    let error: &dyn std::error::Error = &rejection;
    assert!(error.source().is_none());
    assert_eq!(rejection.into_response().status(), StatusCode::INTERNAL_SERVER_ERROR);
}

struct UserAgent<'request>(&'request str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UserAgentRejection;

impl fmt::Display for UserAgentRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the user-agent header is missing or invalid")
    }
}

impl std::error::Error for UserAgentRejection {}

impl IntoResponse for UserAgentRejection {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        StatusCode::BAD_REQUEST.into_response()
    }
}

impl<'request, S: ?Sized> FromRequestParts<'request, S> for UserAgent<'request> {
    type Rejection = UserAgentRejection;

    fn from_request_parts(parts: &'request RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(Self)
            .ok_or(UserAgentRejection)
    }
}

struct Nested<T>(T);

impl<'request, S: ?Sized> FromRequestParts<'request, S> for Nested<UserAgent<'request>> {
    type Rejection = UserAgentRejection;

    fn from_request_parts(parts: &'request RequestParts, state: &S) -> Result<Self, Self::Rejection> {
        UserAgent::from_request_parts(parts, state).map(Self)
    }
}

/// A nested, request-independent extractor written with an explicit `'static`.
///
/// The outermost extractor type is not a reference, so `'static` here names an
/// owned type instead of the request-parts borrow and must be preserved.
impl<S: ?Sized> FromRequestParts<'_, S> for Nested<&'static str> {
    type Rejection = UserAgentRejection;

    fn from_request_parts(_parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self("routerama"))
    }
}

struct AgentsApi {
    calls: Cell<usize>,
}

#[expect(
    clippy::future_not_send,
    reason = "Cell intentionally proves that generated dispatch has no mandatory Send bound"
)]
#[router]
impl AgentsApi {
    #[route(GET, "/static")]
    async fn static_user_agent(&self, user_agent: UserAgent<'_>, headers: &HeaderMap) -> StatusCode {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(
            user_agent.0.as_ptr(),
            headers[USER_AGENT].as_bytes().as_ptr(),
            "the custom extractor must borrow the header value"
        );
        tokio::task::yield_now().await;
        assert_eq!(user_agent.0, "routerama-test");
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/nested")]
    async fn nested(&self, user_agent: Nested<UserAgent<'_>>) -> StatusCode {
        self.calls.set(self.calls.get() + 1);
        tokio::task::yield_now().await;
        assert_eq!(user_agent.0.0, "routerama-test");
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/owned")]
    async fn owned(&self, banner: Nested<&'static str>, user_agent: UserAgent<'_>) -> StatusCode {
        self.calls.set(self.calls.get() + 1);
        tokio::task::yield_now().await;
        assert_eq!(banner.0, "routerama");
        assert_eq!(user_agent.0, "routerama-test");
        StatusCode::NO_CONTENT
    }

    #[route(dynamic)]
    async fn dynamic(&self, user_agent: UserAgent<'_>, #[capture] name: String) -> StatusCode {
        self.calls.set(self.calls.get() + 1);
        tokio::task::yield_now().await;
        assert_eq!(user_agent.0, "routerama-test");
        assert_eq!(name, "plugin");
        StatusCode::NO_CONTENT
    }
}

#[tokio::test]
async fn custom_borrowed_extractors_work_for_static_and_configured_dynamic_routes() {
    let router = AgentsApi::router_builder()
        .add_dynamic("GET", "/dynamic/{name}")
        .build()
        .expect("the dynamic route is valid");
    let api = AgentsApi { calls: Cell::new(0) };

    for path in ["/static", "/nested", "/owned", "/dynamic/plugin"] {
        let request = Request::get(path)
            .header(USER_AGENT, "routerama-test")
            .body(())
            .expect("the test request uses valid static metadata");
        let response = router.route(&api, request, &()).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    assert_eq!(api.calls.get(), 4);
}

#[tokio::test]
async fn custom_borrowed_rejections_short_circuit_before_the_handler() {
    let router = AgentsApi::router_builder()
        .add_dynamic("GET", "/dynamic/{name}")
        .build()
        .expect("the dynamic route is valid");
    let api = AgentsApi { calls: Cell::new(0) };

    for path in ["/static", "/nested", "/dynamic/plugin"] {
        let request = Request::get(path).body(()).expect("the test request uses valid static metadata");
        let response = router.route(&api, request, &()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(api.calls.get(), 0);
}

struct AllocationApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl AllocationApi {
    #[route(GET, "/allocation")]
    async fn allocation(&self, headers: &HeaderMap, uri: &Uri, extensions: &Extensions, parts: &RequestParts) -> StatusCode {
        assert!(std::ptr::eq(std::ptr::from_ref(headers), std::ptr::from_ref(&parts.headers)));
        assert!(std::ptr::eq(std::ptr::from_ref(uri), std::ptr::from_ref(&parts.uri)));
        assert!(std::ptr::eq(std::ptr::from_ref(extensions), std::ptr::from_ref(&parts.extensions)));
        StatusCode::NO_CONTENT
    }
}

#[test]
#[cfg(not(miri))]
fn borrowed_metadata_dispatch_does_not_allocate() {
    let request = Request::get("/allocation")
        .header("x-request", "present")
        .body(())
        .expect("the test request uses valid static metadata");
    let api = AllocationApi;
    let mut future = pin!(api.route(request, &()));
    let mut context = Context::from_waker(Waker::noop());
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("borrowed_metadata_dispatch");

    let response = {
        let _span = operation.measure_thread().iterations(1);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(response) => std::hint::black_box(response),
            Poll::Pending => panic!("the allocation probe has no pending operation"),
        }
    };

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(total_bytes_allocated(&session, "borrowed_metadata_dispatch"), 0);
}
