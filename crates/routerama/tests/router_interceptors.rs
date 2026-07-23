// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for generated before/after/transform interceptors.

#![deny(private_bounds, private_interfaces)]
#![allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::needless_pass_by_ref_mut,
    clippy::future_not_send,
    reason = "router handlers and interceptors must be async, take the macro-required &mut context, and may hold !Send test state; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]

use std::cell::RefCell;
#[cfg(not(miri))]
use std::future::Future as _;
use std::pin::{Pin, pin};
use std::rc::Rc;
#[cfg(not(miri))]
use std::task::Waker;
use std::task::{Context, Poll};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt as _;
use pin_project_lite::pin_project;
use routerama::response::{Body, Response};
use routerama::route::{
    AfterContext, Before, BeforeContext, BodyConsumed, BodyTransform, BytesBody, ClonedExtension, ExtensionRef, Request, RequestParts,
    SelectedContext, StatusCode, router,
};

#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

async fn body_bytes<B>(response: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: core::fmt::Debug,
{
    response.into_body().collect().await.expect("body succeeds").to_bytes()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UserId(u32);

// --- Router-wide + per-handler before, router-wide after ---------------------

struct Api {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl Api {
    #[route(GET, "/open")]
    async fn open(&self, user: ExtensionRef<'_, UserId>) -> String {
        self.log.borrow_mut().push("open");
        format!("open:{}", user.get().0)
    }

    #[route(GET, "/admin")]
    async fn admin(&self, user: ClonedExtension<UserId>) -> String {
        self.log.borrow_mut().push("admin");
        format!("admin:{}", user.0.0)
    }

    #[before]
    async fn authenticate(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("before:router");
        if let Some(key) = ctx.headers().get("x-user") {
            let id: u32 = key.to_str().unwrap_or("0").parse().unwrap_or(0);
            ctx.insert_extension(UserId(id));
        }
        Before::Next
    }

    #[before(admin)]
    async fn require_admin(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("before:admin");
        match ctx.get_extension::<UserId>() {
            Some(_) => Before::Next,
            None => Before::Respond(StatusCode::UNAUTHORIZED),
        }
    }

    #[after]
    async fn stamp(&self, ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after:router");
        ctx.headers_mut().insert("x-handled", "1".parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn router_wide_before_and_after_run_in_order() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let api = Api { log: Rc::clone(&log) };
    let request = Request::get("/open")
        .header("x-user", "7")
        .body(Body::empty())
        .expect("valid request");
    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-handled"], "1");
    assert_eq!(body_bytes(response).await, b"open:7"[..]);
    assert_eq!(*log.borrow(), ["before:router", "open", "after:router"]);
}

#[tokio::test]
async fn per_handler_before_runs_after_router_wide_and_selects_by_route() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let api = Api { log: Rc::clone(&log) };
    let request = Request::get("/admin")
        .header("x-user", "42")
        .body(Body::empty())
        .expect("valid request");
    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"admin:42"[..]);
    // The per-handler `require_admin` guard runs only for `/admin`, after the
    // router-wide `authenticate` interceptor, then the handler, then `stamp`.
    assert_eq!(*log.borrow(), ["before:router", "before:admin", "admin", "after:router"]);
}

#[tokio::test]
async fn open_route_does_not_run_the_admin_guard() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let api = Api { log: Rc::clone(&log) };
    let request = Request::get("/open")
        .header("x-user", "1")
        .body(Body::empty())
        .expect("valid request");
    let _ = api.route(request, &()).await;
    assert_eq!(*log.borrow(), ["before:router", "open", "after:router"]);
}

#[tokio::test]
async fn per_handler_before_short_circuits_the_handler_but_not_the_generated_wide_after() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let api = Api { log: Rc::clone(&log) };
    let request = Request::get("/admin").body(Body::empty()).expect("valid request");
    let response = api.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // The handler never runs, but the bare `#[after]` observes every generated
    // response, including this short-circuit.
    assert_eq!(response.headers()["x-handled"], "1");
    assert_eq!(*log.borrow(), ["before:router", "before:admin", "after:router"]);
}

// --- Ordered multiple router-wide interceptors -------------------------------

struct Ordered {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl Ordered {
    #[route(GET, "/x")]
    async fn handle(&self) -> StatusCode {
        self.log.borrow_mut().push("handle");
        StatusCode::OK
    }

    #[before]
    async fn before_one(&self, _ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("before1");
        Before::Next
    }

    #[before]
    async fn before_two(&self, _ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("before2");
        Before::Next
    }

    #[after]
    async fn after_one(&self, _ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after1");
    }

    #[after]
    async fn after_two(&self, _ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after2");
    }
}

#[tokio::test]
async fn multiple_router_wide_interceptors_run_in_declaration_order() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let ordered = Ordered { log: Rc::clone(&log) };
    let _ = ordered
        .route(Request::get("/x").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(*log.borrow(), ["before1", "before2", "handle", "after1", "after2"]);
}

// --- Body transform + handler extraction, consuming transform ----------------

struct Bodies;

#[router]
impl Bodies {
    #[route(POST, "/upper")]
    async fn upper(&self, #[body] text: BytesBody<64>) -> String {
        String::from_utf8(text.as_bytes().to_vec()).expect("utf8")
    }

    #[route(POST, "/measure")]
    async fn measure(&self) -> String {
        "measured".to_string()
    }

    #[transform(limit = 64, upper)]
    async fn uppercase(&self, ctx: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        assert_eq!(ctx.method, http::Method::POST);
        if body.len() > 32 {
            return BodyTransform::Respond(StatusCode::PAYLOAD_TOO_LARGE);
        }
        BodyTransform::Replace(Body::from_bytes(Bytes::from(body.to_ascii_uppercase())))
    }

    #[transform(limit = 64, measure)]
    async fn only_small(&self, _ctx: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
        if body.len() > 8 {
            BodyConsumed::Respond(StatusCode::PAYLOAD_TOO_LARGE)
        } else {
            BodyConsumed::Consumed
        }
    }
}

#[tokio::test]
async fn transform_replaces_body_then_handler_extracts() {
    let request = Request::post("/upper").body(Body::from("hello")).expect("valid request");
    let response = Bodies.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"HELLO"[..]);
}

#[tokio::test]
async fn transform_short_circuits() {
    let request = Request::post("/upper")
        .body(Body::from("this body is definitely longer than the transform limit"))
        .expect("valid request");
    let response = Bodies.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn consuming_transform_runs_without_handler_body() {
    let ok = Bodies
        .route(Request::post("/measure").body(Body::from("tiny")).expect("valid request"), &())
        .await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(body_bytes(ok).await, b"measured"[..]);

    let big = Bodies
        .route(
            Request::post("/measure")
                .body(Body::from("way too many bytes"))
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(big.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

struct SharedBodyExtractor;

#[router]
impl SharedBodyExtractor {
    #[route(POST, "/transformed")]
    async fn transformed(&self, #[body] body: BytesBody<64>) -> Bytes {
        body.into_inner()
    }

    #[route(POST, "/original")]
    async fn original(&self, #[body] body: BytesBody<64>) -> Bytes {
        body.into_inner()
    }

    #[transform(limit = 64, transformed)]
    async fn replace(&self, _ctx: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        BodyTransform::Replace(Body::from_bytes(body))
    }
}

#[tokio::test]
async fn shared_body_extractor_supports_transformed_and_original_input_types() {
    for path in ["/transformed", "/original"] {
        let response = SharedBodyExtractor
            .route(Request::post(path).body(Body::from("same")).expect("valid request"), &())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, b"same"[..]);
    }
}

struct TransformOrder {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl TransformOrder {
    #[route(POST, "/ordered")]
    async fn ordered(&self, missing: ExtensionRef<'_, UserId>) -> String {
        missing.get().0.to_string()
    }

    #[transform(limit = 64, ordered)]
    async fn audit(&self, _ctx: &RequestParts, _body: Bytes) -> BodyConsumed<StatusCode> {
        self.log.borrow_mut().push("transform");
        BodyConsumed::Consumed
    }
}

#[tokio::test]
async fn transform_runs_before_request_parts_extraction() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let response = TransformOrder { log: Rc::clone(&log) }
        .route(Request::post("/ordered").body(Body::from("audited")).expect("valid request"), &())
        .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(*log.borrow(), ["transform"]);
}

// --- Configured dynamic router -----------------------------------------------

struct Configured {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl Configured {
    #[route(dynamic)]
    async fn plugin(&self, #[capture] name: String) -> String {
        self.log.borrow_mut().push("plugin");
        name
    }

    #[before]
    async fn trace(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("before");
        ctx.insert_extension(UserId(9));
        Before::Next
    }

    #[before(plugin)]
    async fn guard_plugin(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("guard");
        if ctx.get_extension::<UserId>().is_some() {
            Before::Next
        } else {
            Before::Respond(StatusCode::FORBIDDEN)
        }
    }

    #[after]
    async fn seal(&self, ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after");
        ctx.headers_mut().insert("x-dynamic", "1".parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn configured_dynamic_router_runs_interceptors() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let service = Configured { log: Rc::clone(&log) };
    let router = Configured::router_builder()
        .add_plugin("GET", "/plugins/{name}")
        .build()
        .expect("dynamic registration is valid");
    let request = Request::get("/plugins/search").body(Body::empty()).expect("valid request");
    let response = router.route(&service, request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-dynamic"], "1");
    assert_eq!(body_bytes(response).await, b"search"[..]);
    assert_eq!(*log.borrow(), ["before", "guard", "plugin", "after"]);
}

// --- Fixed-state router ------------------------------------------------------

#[derive(Clone)]
struct AppState {
    tag: &'static str,
}

struct Fixed;

#[router(state = AppState)]
impl Fixed {
    #[route(POST, "/echo")]
    async fn echo(&self, #[body] text: BytesBody<64>, state: routerama::route::State<AppState>) -> String {
        format!("{}:{}", state.tag, String::from_utf8(text.as_bytes().to_vec()).expect("utf8"))
    }

    #[before]
    async fn tag(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        ctx.insert_extension(UserId(1));
        Before::Next
    }

    #[transform(limit = 64, echo)]
    async fn reverse(&self, _ctx: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        let mut bytes = body.to_vec();
        bytes.reverse();
        BodyTransform::Replace(Body::from_bytes(Bytes::from(bytes)))
    }

    #[after]
    async fn brand(&self, ctx: &mut AfterContext<'_>) {
        let id = ctx.request().extensions.get::<UserId>().expect("before inserted the id").0;
        ctx.headers_mut()
            .insert("x-fixed", id.to_string().parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn fixed_state_router_supports_interceptors_and_transform() {
    let state = AppState { tag: "fixed" };
    let request = Request::post("/echo").body(Body::from("abc")).expect("valid request");
    let response = Fixed.route(request, &state).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-fixed"], "1");
    assert_eq!(body_bytes(response).await, b"fixed:cba"[..]);
}

// --- Streaming response preserved through after ------------------------------

struct Chunks {
    frames: Vec<Bytes>,
}

impl http_body::Body for Chunks {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.frames.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(Ok(Frame::data(self.frames.remove(0)))))
        }
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

struct Streaming;

#[router]
impl Streaming {
    #[route(GET, "/stream")]
    async fn stream(&self) -> Response<Chunks> {
        Response::new(Chunks {
            frames: vec![Bytes::from_static(b"one"), Bytes::from_static(b"two")],
        })
    }

    #[after]
    async fn trailer(&self, ctx: &mut AfterContext<'_>) {
        ctx.headers_mut().insert("x-stream", "1".parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn after_preserves_a_streaming_body() {
    let response = Streaming
        .route(Request::get("/stream").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.headers()["x-stream"], "1");
    assert_eq!(body_bytes(response).await, b"onetwo"[..]);
}

// --- produces + after interaction --------------------------------------------

struct Negotiated;

#[router]
impl Negotiated {
    #[route(GET, "/data", produces = "application/json")]
    async fn data(&self) -> String {
        "{}".to_string()
    }

    #[after]
    async fn override_type(&self, ctx: &mut AfterContext<'_>) {
        ctx.headers_mut()
            .insert("content-type", "application/problem+json".parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn after_runs_after_produces_content_type() {
    let request = Request::get("/data")
        .header("accept", "application/json")
        .body(Body::empty())
        .expect("valid request");
    let response = Negotiated.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    // `produces` sets `application/json`; the after interceptor overrides it.
    assert_eq!(response.headers()["content-type"], "application/problem+json");
}

// --- Interceptors compose with priority overlap route selection --------------

struct Overlap {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl Overlap {
    #[route(GET, "/reports/{id}", produces = "application/json", priority = 10)]
    async fn json(&self, id: u32) -> String {
        self.log.borrow_mut().push("json");
        format!(r#"{{"id":{id}}}"#)
    }

    #[route(GET, "/reports/{id}", produces = "text/plain", priority = 0)]
    async fn text(&self, id: u32) -> String {
        self.log.borrow_mut().push("text");
        format!("report {id}")
    }

    #[before]
    async fn trace(&self, _ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("before");
        Before::Next
    }

    #[before(json)]
    async fn guard_json(&self, _ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("guard");
        Before::Next
    }

    #[after]
    async fn seal(&self, ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after");
        ctx.headers_mut().insert("x-overlap", "1".parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn interceptors_compose_with_priority_overlap() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let overlap = Overlap { log: Rc::clone(&log) };
    let request = Request::get("/reports/42")
        .header("accept", "application/json")
        .body(Body::empty())
        .expect("valid request");
    let response = overlap.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["x-overlap"], "1");
    assert_eq!(body_bytes(response).await, br#"{"id":42}"#[..]);
    // Router-wide before, then the selected candidate's per-handler before,
    // then the handler, then after.
    assert_eq!(*log.borrow(), ["before", "guard", "json", "after"]);
}

// --- Per-handler guards compose with zero-copy borrowed captures -------------

struct Zero {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl Zero {
    /// Borrows `slug` directly out of the request URI *and* reads an extension
    /// the per-handler guard inserted, with no owned capture in between.
    #[route(GET, "/books/{slug}")]
    async fn book(&self, slug: &str, caller: ExtensionRef<'_, UserId>) -> String {
        self.log.borrow_mut().push("book");
        format!("{slug}:{}", caller.get().0)
    }

    #[route(GET, "/books/{slug}/reviews/{id}")]
    async fn review(&self, slug: &str, id: u32, caller: ExtensionRef<'_, UserId>) -> String {
        self.log.borrow_mut().push("review");
        format!("{slug}/{id}:{}", caller.get().0)
    }

    #[before(book, review)]
    async fn authenticate(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        self.log.borrow_mut().push("guard");
        assert_eq!(ctx.method(), http::Method::GET);
        assert_eq!(ctx.version(), http::Version::HTTP_11);
        assert!(ctx.uri().path().starts_with("/books/"));
        let Some(user) = ctx.headers().get("x-user").and_then(|value| value.to_str().ok()) else {
            return Before::Respond(StatusCode::UNAUTHORIZED);
        };
        let id = user.parse().unwrap_or(0);
        ctx.headers_mut().insert("x-checked", "1".parse().expect("valid header value"));
        assert_eq!(ctx.insert_extension(UserId(id)), None);
        assert_eq!(ctx.get_extension::<UserId>(), Some(&UserId(id)));
        Before::Next
    }
}

#[tokio::test]
async fn per_handler_guard_enriches_a_handler_with_borrowed_captures() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let zero = Zero { log: Rc::clone(&log) };
    let request = Request::get("/books/rust-in-action")
        .header("x-user", "13")
        .body(Body::empty())
        .expect("valid request");
    let response = zero.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"rust-in-action:13"[..]);
    assert_eq!(*log.borrow(), ["guard", "book"]);
}

#[tokio::test]
async fn per_handler_guard_short_circuits_a_borrowed_capture_handler() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let zero = Zero { log: Rc::clone(&log) };
    let request = Request::get("/books/rust-in-action/reviews/4")
        .body(Body::empty())
        .expect("valid request");
    let response = zero.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(*log.borrow(), ["guard"]);
}

#[tokio::test]
async fn per_handler_guard_composes_with_mixed_borrowed_and_parsed_captures() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let zero = Zero { log: Rc::clone(&log) };
    let request = Request::get("/books/rust-in-action/reviews/4")
        .header("x-user", "8")
        .body(Body::empty())
        .expect("valid request");
    let response = zero.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"rust-in-action/4:8"[..]);
    assert_eq!(*log.borrow(), ["guard", "review"]);
}

// --- Streaming transforms ----------------------------------------------------

/// A multi-frame request body that records how many frames were polled, so a
/// test can prove a streaming transform never buffered it.
struct Frames {
    frames: Vec<Bytes>,
    polls: Rc<RefCell<Vec<usize>>>,
}

impl Frames {
    fn new(frames: &[&'static [u8]], polls: &Rc<RefCell<Vec<usize>>>) -> Self {
        Self {
            frames: frames.iter().map(|frame| Bytes::from_static(frame)).collect(),
            polls: Rc::clone(polls),
        }
    }
}

impl http_body::Body for Frames {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.frames.is_empty() {
            return Poll::Ready(None);
        }
        let frame = self.frames.remove(0);
        self.polls.borrow_mut().push(frame.len());
        Poll::Ready(Some(Ok(Frame::data(frame))))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pin_project! {
    /// A streaming wrapper that records each frame as it passes, without ever
    /// collecting the body. `!Send` because it shares an `Rc` tally.
    struct Tap<B> {
        #[pin]
        inner: B,
        seen: Rc<RefCell<Vec<usize>>>,
    }
}

impl<B> http_body::Body for Tap<B>
where
    B: http_body::Body<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let polled = this.inner.poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let Some(data) = frame.data_ref()
        {
            this.seen.borrow_mut().push(data.len());
        }
        polled
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

struct Wrapped {
    seen: Rc<RefCell<Vec<usize>>>,
    drained: Rc<RefCell<usize>>,
}

#[router]
impl Wrapped {
    /// The `#[body]` parameter extracts from `Tap<__RouteramaBody>`, the exact
    /// wrapper the streaming transform returned.
    #[route(POST, "/wrapped")]
    async fn wrapped(&self, #[body] body: BytesBody<64>) -> Bytes {
        body.into_inner()
    }

    #[route(POST, "/drained")]
    async fn drained(&self) -> String {
        format!("drained {}", self.drained.borrow())
    }

    /// Wraps the transport body lazily: nothing is buffered, and the returned
    /// wrapper is what later `#[body]` extraction sees.
    #[transform(stream, wrapped)]
    async fn tap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<Tap<B>, StatusCode>
    where
        B: http_body::Body<Data = Bytes> + Unpin,
    {
        if parts.headers.contains_key("x-reject") {
            return BodyTransform::Respond(StatusCode::FORBIDDEN);
        }
        BodyTransform::Replace(Tap {
            inner: body,
            seen: Rc::clone(&self.seen),
        })
    }

    /// A streaming terminal consumer: it drains frame by frame and never
    /// buffers, so its handler declares no `#[body]` parameter.
    #[transform(stream, drained)]
    async fn drain<B>(&self, _parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
    where
        B: http_body::Body<Data = Bytes>,
    {
        let mut body = pin!(body);
        let mut total = 0_usize;
        while let Some(frame) = core::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
            let Ok(frame) = frame else {
                return BodyConsumed::Respond(StatusCode::BAD_REQUEST);
            };
            total += frame.data_ref().map_or(0, Bytes::len);
        }
        *self.drained.borrow_mut() = total;
        BodyConsumed::Consumed
    }
}

#[tokio::test]
async fn streaming_transform_wraps_the_body_for_handler_extraction() {
    let polls = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let service = Wrapped {
        seen: Rc::clone(&seen),
        drained: Rc::new(RefCell::new(0)),
    };
    let request = Request::post("/wrapped")
        .body(Frames::new(&[b"one", b"two!"], &polls))
        .expect("valid request");
    let response = service.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"onetwo!"[..]);
    // The wrapper observed each frame separately, so the framework buffered
    // nothing before handing the replacement body to `#[body]` extraction.
    assert_eq!(*seen.borrow(), [3, 4]);
    assert_eq!(*polls.borrow(), [3, 4]);
}

#[tokio::test]
async fn streaming_transform_short_circuits_without_reading_the_body() {
    let polls = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let service = Wrapped {
        seen: Rc::clone(&seen),
        drained: Rc::new(RefCell::new(0)),
    };
    let request = Request::post("/wrapped")
        .header("x-reject", "1")
        .body(Frames::new(&[b"one", b"two!"], &polls))
        .expect("valid request");
    let response = service.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(polls.borrow().is_empty(), "a short-circuit never polls the request body");
    assert!(seen.borrow().is_empty());
}

#[tokio::test]
async fn streaming_consuming_transform_drains_without_buffering() {
    let polls = Rc::new(RefCell::new(Vec::new()));
    let drained = Rc::new(RefCell::new(0));
    let service = Wrapped {
        seen: Rc::new(RefCell::new(Vec::new())),
        drained: Rc::clone(&drained),
    };
    let request = Request::post("/drained")
        .body(Frames::new(&[b"alpha", b"beta"], &polls))
        .expect("valid request");
    let response = service.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"drained 9"[..]);
    assert_eq!(*polls.borrow(), [5, 4]);
    assert_eq!(*drained.borrow(), 9);
}

/// A streaming transform on a fixed-state service, proving the substituted
/// wrapper type also satisfies the eager fixed-state contract.
struct FixedStream;

#[router(state = AppState)]
impl FixedStream {
    #[route(POST, "/tagged")]
    async fn tagged(&self, #[body] body: BytesBody<64>, state: routerama::route::State<AppState>) -> String {
        format!("{}:{}", state.tag, String::from_utf8(body.as_bytes().to_vec()).expect("utf8"))
    }

    #[transform(stream, tagged)]
    async fn passthrough<B>(&self, _parts: &RequestParts, body: B) -> BodyTransform<Tap<B>, StatusCode>
    where
        B: http_body::Body<Data = Bytes>,
    {
        BodyTransform::Replace(Tap {
            inner: body,
            seen: Rc::new(RefCell::new(Vec::new())),
        })
    }
}

#[tokio::test]
async fn fixed_state_router_supports_a_streaming_transform() {
    let state = AppState { tag: "fixed" };
    let request = Request::post("/tagged").body(Body::from("abc")).expect("valid request");
    let response = FixedStream.route(request, &state).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"fixed:abc"[..]);
}

// --- Exact `#[after]` scope --------------------------------------------------

struct Scoped {
    log: Rc<RefCell<Vec<&'static str>>>,
}

#[router]
impl Scoped {
    #[route(GET, "/ok")]
    async fn ok(&self) -> StatusCode {
        StatusCode::OK
    }

    #[route(GET, "/json", produces = "application/json")]
    async fn json(&self) -> String {
        "{}".to_string()
    }

    #[route(GET, "/extension")]
    async fn extension(&self, caller: ExtensionRef<'_, UserId>) -> String {
        caller.get().0.to_string()
    }

    #[route(GET, "/guarded")]
    async fn guarded(&self) -> StatusCode {
        StatusCode::OK
    }

    #[route(POST, "/bounded")]
    async fn bounded(&self, #[body] body: BytesBody<8>) -> Bytes {
        body.into_inner()
    }

    #[before(guarded)]
    async fn deny(&self, _ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        Before::Respond(StatusCode::FORBIDDEN)
    }

    #[transform(limit = 8, bounded)]
    async fn refuse(&self, _parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        if body.starts_with(b"no") {
            return BodyTransform::Respond(StatusCode::CONFLICT);
        }
        BodyTransform::Replace(Body::from_bytes(body))
    }

    /// Runs only for `ok`, and only when that handler actually returned.
    #[after(ok)]
    async fn only_ok(&self, ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after:ok");
        ctx.headers_mut().insert("x-handler", "ok".parse().expect("valid header value"));
    }

    /// Observes every response this router generates.
    #[after]
    async fn seal(&self, ctx: &mut AfterContext<'_>) {
        self.log.borrow_mut().push("after:all");
        let path = ctx.request().uri.path().to_owned();
        ctx.headers_mut().insert("x-path", path.parse().expect("valid header value"));
        ctx.headers_mut().insert("x-sealed", "1".parse().expect("valid header value"));
    }
}

async fn sealed(path: &str, request: Request<Body>) -> (StatusCode, Vec<&'static str>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let response = Scoped { log: Rc::clone(&log) }.route(request, &()).await;
    assert_eq!(response.headers()["x-sealed"], "1", "every generated response is observed");
    assert_eq!(
        response.headers()["x-path"],
        path,
        "the after interceptor still reads the request head"
    );
    let status = response.status();
    let calls = log.borrow().clone();
    (status, calls)
}

#[tokio::test]
async fn generated_wide_after_observes_every_generated_response() {
    // A handler response, plus its own per-handler interceptor.
    assert_eq!(
        sealed("/ok", Request::get("/ok").body(Body::empty()).expect("valid request")).await,
        (StatusCode::OK, vec!["after:ok", "after:all"])
    );
    // A routing failure.
    assert_eq!(
        sealed("/missing", Request::get("/missing").body(Body::empty()).expect("valid request")).await,
        (StatusCode::NOT_FOUND, vec!["after:all"])
    );
    // A route predicate rejection.
    assert_eq!(
        sealed(
            "/json",
            Request::get("/json")
                .header("accept", "text/plain")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await,
        (StatusCode::NOT_ACCEPTABLE, vec!["after:all"])
    );
    // A request-parts extractor rejection.
    assert_eq!(
        sealed("/extension", Request::get("/extension").body(Body::empty()).expect("valid request")).await,
        (StatusCode::INTERNAL_SERVER_ERROR, vec!["after:all"])
    );
    // A per-handler `#[before]` short-circuit.
    assert_eq!(
        sealed("/guarded", Request::get("/guarded").body(Body::empty()).expect("valid request")).await,
        (StatusCode::FORBIDDEN, vec!["after:all"])
    );
    // A `#[transform]` short-circuit.
    assert_eq!(
        sealed("/bounded", Request::post("/bounded").body(Body::from("no")).expect("valid request")).await,
        (StatusCode::CONFLICT, vec!["after:all"])
    );
    // A transform buffering rejection.
    assert_eq!(
        sealed(
            "/bounded",
            Request::post("/bounded")
                .body(Body::from("far too many bytes"))
                .expect("valid request"),
        )
        .await,
        (StatusCode::PAYLOAD_TOO_LARGE, vec!["after:all"])
    );
    // A request-body extractor rejection reached through a transform replacement.
    assert_eq!(
        sealed(
            "/bounded",
            Request::post("/bounded").body(Body::from("exactly!")).expect("valid request")
        )
        .await,
        (StatusCode::OK, vec!["after:all"])
    );
}

#[tokio::test]
async fn per_handler_after_runs_only_for_its_own_handler_response() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let scoped = Scoped { log: Rc::clone(&log) };
    let response = scoped
        .route(Request::get("/ok").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.headers()["x-handler"], "ok");

    let response = scoped
        .route(Request::get("/guarded").body(Body::empty()).expect("valid request"), &())
        .await;
    assert!(
        !response.headers().contains_key("x-handler"),
        "a per-handler after never observes another route's short-circuit"
    );
}

/// A typed fallback response is generated, so it is observed too.
struct Fallback;

#[router]
impl Fallback {
    #[route(GET, "/present")]
    async fn present(&self) -> StatusCode {
        StatusCode::OK
    }

    #[fallback]
    async fn missing(&self, failure: routerama::route::RouteFailure<'_>) -> (StatusCode, String) {
        (failure.status(), format!("no route for {}", failure.path().unwrap_or("?")))
    }

    #[after]
    async fn seal(&self, ctx: &mut AfterContext<'_>) {
        ctx.headers_mut().insert("x-sealed", "1".parse().expect("valid header value"));
    }
}

#[tokio::test]
async fn generated_wide_after_observes_a_typed_fallback_response() {
    let response = Fallback
        .route(Request::get("/absent").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["x-sealed"], "1");
    assert_eq!(body_bytes(response).await, b"no route for /absent"[..]);
}

// --- Allocation and layout ---------------------------------------------------

struct Passive;

#[router]
impl Passive {
    #[route(GET, "/p")]
    async fn handle(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[before]
    async fn peek(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _ = ctx.headers().len();
        Before::Next
    }

    #[after]
    async fn observe(&self, ctx: &mut AfterContext<'_>) {
        let _ = ctx.status();
    }
}

#[test]
#[cfg(not(miri))]
fn passive_interceptors_add_no_allocation_on_the_generated_path() {
    let session = Session::new().no_stdout().no_file();
    let mut context = Context::from_waker(Waker::noop());
    let request = Request::get("/p").body(Body::empty()).expect("valid request");
    let mut future = pin!(Passive.route(request, &()));
    let operation = session.operation("passive_interceptors");
    let response = {
        let _span = operation.measure_thread();
        match future.as_mut().poll(&mut context) {
            Poll::Ready(response) => std::hint::black_box(response),
            Poll::Pending => panic!("prepared interceptor dispatch has no pending operation"),
        }
    };
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(operation.total_bytes_allocated(), 0);
}

/// A guarded, streaming, observed route whose stages must all stay
/// allocation-free: a borrowed capture, a per-handler guard, a streaming
/// terminal consumer, and a generated-wide response interceptor.
struct Lean;

#[router]
impl Lean {
    #[route(POST, "/lean/{tag}")]
    async fn lean(&self, tag: &str) -> StatusCode {
        if tag.is_empty() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NO_CONTENT
        }
    }

    #[before(lean)]
    async fn guard(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        let _ = ctx.headers().len();
        Before::Next
    }

    #[transform(stream, lean)]
    async fn wrap<B>(&self, _parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
    where
        B: http_body::Body<Data = Bytes>,
    {
        drop(body);
        BodyConsumed::Consumed
    }

    #[after]
    async fn observe(&self, ctx: &mut AfterContext<'_>) {
        let _ = ctx.status();
    }
}

#[test]
#[cfg(not(miri))]
fn streaming_transform_and_guard_add_no_allocation_on_the_generated_path() {
    let session = Session::new().no_stdout().no_file();
    let mut context = Context::from_waker(Waker::noop());
    let request = Request::post("/lean/alpha").body(Body::empty()).expect("valid request");
    let mut future = pin!(Lean.route(request, &()));
    let operation = session.operation("streaming_interceptors");
    let response = {
        let _span = operation.measure_thread();
        match future.as_mut().poll(&mut context) {
            Poll::Ready(response) => std::hint::black_box(response),
            Poll::Pending => panic!("prepared interceptor dispatch has no pending operation"),
        }
    };
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(operation.total_bytes_allocated(), 0);
}

// --- Mounted services receive router-wide before interceptors ----------------

#[cfg(feature = "mount")]
mod mounts {
    use routerama::route::mount::{ErasedMountRouter, ErasedMountService, MountedRequest};

    use super::{AfterContext, Before, BeforeContext, Body, Request, StatusCode, UserId, body_bytes, router};

    struct Gateway;

    #[router(state = (), erased_mounts)]
    impl Gateway {
        #[route(GET, "/health")]
        async fn health(&self) -> StatusCode {
            StatusCode::NO_CONTENT
        }

        #[before]
        async fn authenticate(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
            if ctx.headers().contains_key("x-user") {
                ctx.insert_extension(UserId(5));
                Before::Next
            } else {
                Before::Respond(StatusCode::UNAUTHORIZED)
            }
        }

        #[after]
        async fn seal(&self, ctx: &mut AfterContext<'_>) {
            ctx.headers_mut().insert("x-generated", "1".parse().expect("valid header value"));
        }
    }

    fn mounts() -> ErasedMountRouter<Body, ()> {
        ErasedMountRouter::builder()
            .mount(
                "GET",
                "/plugins/{name}",
                ErasedMountService::<Body, ()>::from_async_fn(async |request: MountedRequest<'_, Body>, _state: &()| {
                    let id = request.request().extensions().get::<UserId>().copied();
                    let name = request.decoded_capture("name").expect("template captures name").into_owned();
                    Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from(format!("{}:{}", id.map_or(0, |id| id.0), name)))
                        .expect("valid response")
                }),
            )
            .build()
            .expect("mount registration is valid")
    }

    use routerama::response::Response;

    #[tokio::test]
    async fn router_wide_before_enriches_a_mounted_request() {
        let mounts = mounts();
        let request = Request::get("/plugins/search")
            .header("x-user", "1")
            .body(Body::empty())
            .expect("valid request");
        let response = Gateway.route_with_erased_mounts(request, &(), &mounts).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        // The router-wide `authenticate` interceptor ran before delegation and
        // inserted the identity the mounted service observed.
        assert_eq!(body_bytes(response).await, b"5:search"[..]);
    }

    #[tokio::test]
    async fn router_wide_before_short_circuits_a_mounted_request() {
        let mounts = mounts();
        let request = Request::get("/plugins/search").body(Body::empty()).expect("valid request");
        let response = Gateway.route_with_erased_mounts(request, &(), &mounts).await;
        // No `x-user`: the interceptor short-circuited before the mount ran.
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn mounted_response_is_not_observed_by_after() {
        let mounts = mounts();
        let request = Request::get("/plugins/search")
            .header("x-user", "1")
            .body(Body::empty())
            .expect("valid request");
        let response = Gateway.route_with_erased_mounts(request, &(), &mounts).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        // The mounted service owns the request head from delegation onwards, so
        // `#[after]` deliberately observes only generated responses.
        assert!(
            !response.headers().contains_key("x-generated"),
            "a mounted service response is not a generated response"
        );
    }

    #[tokio::test]
    async fn generated_routing_failure_still_runs_after_interceptor() {
        // The mount table is the final backstop, so a complete miss is answered
        // by the mount router and is likewise not a generated response.
        let mounts = mounts();
        let request = Request::get("/absent")
            .header("x-user", "1")
            .body(Body::empty())
            .expect("valid request");
        let response = Gateway.route_with_erased_mounts(request, &(), &mounts).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response.headers().contains_key("x-generated"));
    }

    #[tokio::test]
    async fn generated_hit_still_runs_after_interceptor() {
        let mounts = mounts();
        let request = Request::get("/health")
            .header("x-user", "1")
            .body(Body::empty())
            .expect("valid request");
        let response = Gateway.route_with_erased_mounts(request, &(), &mounts).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["x-generated"], "1");
    }
}
