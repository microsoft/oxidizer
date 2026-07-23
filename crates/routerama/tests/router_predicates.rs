// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral and allocation coverage for generated HTTP route predicates.

use std::cell::{Cell, RefCell};
use std::fmt;
#[cfg(not(miri))]
use std::future::Future as _;
#[cfg(not(miri))]
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(not(miri))]
use std::task::Waker;
use std::task::{Context, Poll};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http::header::{ACCEPT, CONTENT_TYPE, HOST, HeaderName, HeaderValue};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::{FromRequestBody, FromRequestParts, HeaderMap, RawBody, Request, RequestParts, State, StatusCode, router};

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

struct PredicateApi {
    calls: Cell<usize>,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "Cell records direct handler calls and router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PredicateApi {
    #[route(GET, "/host", host = "api.example:443")]
    async fn host(&self) -> StatusCode {
        self.called()
    }

    #[route(GET, "/ipv6", host = "[2001:db8::1]:8443")]
    async fn ipv6(&self) -> StatusCode {
        self.called()
    }

    #[route(POST, "/consumes", consumes = "application/json")]
    async fn consumes(&self) -> StatusCode {
        self.called()
    }

    #[route(GET, "/produces", produces = "application/json")]
    async fn produces(&self) -> Response<TrailerBody> {
        self.calls.set(self.calls.get() + 1);
        Response::builder()
            .header(CONTENT_TYPE, "text/plain")
            .header(CONTENT_TYPE, "application/problem+json")
            .body(TrailerBody::new())
            .expect("static response metadata is valid")
    }

    #[route(GET, "/application-error", produces = "application/problem+json")]
    async fn application_error(&self) -> StatusCode {
        self.calls.set(self.calls.get() + 1);
        StatusCode::BAD_REQUEST
    }

    #[route(GET, "/alias", host = "aliases.example", produces = "text/plain")]
    #[route(HEAD, "/alias", produces = "text/plain", host = "aliases.example")]
    async fn alias(&self) -> &'static str {
        self.calls.set(self.calls.get() + 1);
        "alias"
    }
}

impl PredicateApi {
    fn called(&self) -> StatusCode {
        self.calls.set(self.calls.get() + 1);
        StatusCode::NO_CONTENT
    }
}

#[tokio::test]
async fn host_uses_uri_authority_first_and_compares_the_complete_value() {
    let api = PredicateApi { calls: Cell::new(0) };

    let request = request_with_headers(MethodUri::Get("http://API.EXAMPLE:443/host"), &[(HOST, "wrong.example")], ());
    assert_eq!(api.route(request, &()).await.status(), StatusCode::NO_CONTENT);

    let request = request_with_headers(MethodUri::Get("http://wrong.example/host"), &[(HOST, "api.example:443")], ());
    assert_eq!(api.route(request, &()).await.status(), StatusCode::NOT_FOUND);

    for host in ["API.EXAMPLE:443", "api.example:443"] {
        let request = request_with_headers(MethodUri::Get("/host"), &[(HOST, host)], ());
        assert_eq!(api.route(request, &()).await.status(), StatusCode::NO_CONTENT);
    }
    for host in ["api.example", "api.example:80", "api example:443", "user@api.example:443"] {
        let request = request_with_headers(MethodUri::Get("/host"), &[(HOST, host)], ());
        assert_eq!(api.route(request, &()).await.status(), StatusCode::NOT_FOUND, "{host}");
    }
    assert_eq!(
        api.route(Request::get("/host").body(()).expect("valid request"), &())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let duplicate = Request::get("/host")
        .header(HOST, "api.example:443")
        .header(HOST, "api.example:443")
        .body(())
        .expect("valid request");
    assert_eq!(api.route(duplicate, &()).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn host_supports_bracketed_ipv6_and_explicit_ports() {
    let api = PredicateApi { calls: Cell::new(0) };
    let relative = request_with_headers(MethodUri::Get("/ipv6"), &[(HOST, "[2001:DB8::1]:8443")], ());
    assert_eq!(api.route(relative, &()).await.status(), StatusCode::NO_CONTENT);
    let absolute = request_with_headers(MethodUri::Get("http://[2001:DB8::1]:8443/ipv6"), &[], ());
    assert_eq!(api.route(absolute, &()).await.status(), StatusCode::NO_CONTENT);
    for host in ["2001:db8::1", "[2001:db8::1]", "[2001:db8::1]:443"] {
        let request = request_with_headers(MethodUri::Get("/ipv6"), &[(HOST, host)], ());
        assert_eq!(api.route(request, &()).await.status(), StatusCode::NOT_FOUND, "{host}");
    }
}

#[tokio::test]
async fn consumes_accepts_case_ows_and_legal_parameters_but_rejects_every_other_shape() {
    let api = PredicateApi { calls: Cell::new(0) };
    for content_type in [
        "application/json",
        "Application/JSON",
        " application/json ; charset=utf-8 ",
        "application/json;charset=\"utf-8\"",
    ] {
        let request = request_with_headers(MethodUri::Post("/consumes"), &[(CONTENT_TYPE, content_type)], ());
        assert_eq!(api.route(request, &()).await.status(), StatusCode::NO_CONTENT, "{content_type}");
    }

    let missing = Request::post("/consumes").body(()).expect("valid request");
    assert_eq!(api.route(missing, &()).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    for content_type in [
        "text/plain",
        "application",
        "application /json",
        "application/json, text/plain",
        "application/json; charset",
        "application/json; charset=\"unterminated",
    ] {
        let request = request_with_headers(MethodUri::Post("/consumes"), &[(CONTENT_TYPE, content_type)], ());
        assert_eq!(
            api.route(request, &()).await.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{content_type}"
        );
    }

    let duplicate = Request::post("/consumes")
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .body(())
        .expect("valid request");
    assert_eq!(api.route(duplicate, &()).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn produces_negotiates_all_ranges_qualities_and_field_lines() {
    let api = PredicateApi { calls: Cell::new(0) };
    let accepted = [
        None,
        Some("application/json"),
        Some("application/*"),
        Some("*/*"),
        Some("text/plain, application/json;q=0.2"),
        Some("application/json;q=0.5;extension=ok"),
    ];
    for accept in accepted {
        let request = match accept {
            Some(value) => request_with_headers(MethodUri::Get("/produces"), &[(ACCEPT, value)], ()),
            None => request_with_headers(MethodUri::Get("/produces"), &[], ()),
        };
        assert_eq!(api.route(request, &()).await.status(), StatusCode::OK, "{accept:?}");
    }

    let multiple = Request::get("/produces")
        .header(ACCEPT, "text/plain")
        .header(ACCEPT, "application/json;q=0.4")
        .body(())
        .expect("valid request");
    assert_eq!(api.route(multiple, &()).await.status(), StatusCode::OK);

    for accept in [
        "text/plain",
        "*/*;q=1, application/json;q=0",
        "application/*;q=1, application/json;q=0",
        "application/json;q=0",
        "application/json;q=2",
        "application/json;q=.5",
        "application/json;version=2",
        "*/json",
        "application/json trailing",
    ] {
        let request = request_with_headers(MethodUri::Get("/produces"), &[(ACCEPT, accept)], ());
        let response = api.route(request, &()).await;
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{accept}");
        assert!(
            !response.headers().contains_key(CONTENT_TYPE),
            "predicate rejections must not receive produced response metadata"
        );
    }
}

#[tokio::test]
async fn produces_replaces_content_type_and_preserves_stream_frames_and_trailers() {
    let api = PredicateApi { calls: Cell::new(0) };
    let request = request_with_headers(MethodUri::Get("/produces"), &[(ACCEPT, "application/json")], ());
    let response = api.route(request, &()).await;
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(response.headers().get_all(CONTENT_TYPE).iter().count(), 1);

    let mut body = response.into_body();
    let data = body.frame().await.expect("data frame").expect("stream succeeds");
    assert_eq!(data.into_data().expect("first frame is data"), b"stream"[..]);
    let trailers = body
        .frame()
        .await
        .expect("trailer frame")
        .expect("stream succeeds")
        .into_trailers()
        .expect("second frame is trailers");
    assert_eq!(trailers["x-stream-complete"], "yes");
    assert!(body.frame().await.is_none());

    let application_error = request_with_headers(MethodUri::Get("/application-error"), &[(ACCEPT, "application/problem+json")], ());
    let response = api.route(application_error, &()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
}

#[tokio::test]
async fn identical_static_aliases_share_their_predicates() {
    let api = PredicateApi { calls: Cell::new(0) };
    for method in [http::Method::GET, http::Method::HEAD] {
        let request = Request::builder()
            .method(method)
            .uri("/alias")
            .header(HOST, "ALIASES.EXAMPLE")
            .header(ACCEPT, "text/plain")
            .body(())
            .expect("valid request");
        let response = api.route(request, &()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/plain");
    }
}

#[derive(Default)]
struct ProbeState {
    reject_parts: AtomicBool,
    parts: AtomicUsize,
    bodies: AtomicUsize,
    handlers: AtomicUsize,
}

struct ProbeParts;

impl<'request> FromRequestParts<'request, Arc<ProbeState>> for ProbeParts {
    type Rejection = StatusCode;

    fn from_request_parts(_parts: &'request RequestParts, state: &Arc<ProbeState>) -> Result<Self, Self::Rejection> {
        state.parts.fetch_add(1, Ordering::SeqCst);
        if state.reject_parts.load(Ordering::SeqCst) {
            Err(StatusCode::BAD_REQUEST)
        } else {
            Ok(Self)
        }
    }
}

struct ProbeBody(Vec<u8>);

impl FromRequestBody<Arc<ProbeState>, Vec<u8>> for ProbeBody {
    type Rejection = StatusCode;

    fn from_request_body(
        _parts: &RequestParts,
        body: Vec<u8>,
        state: &Arc<ProbeState>,
    ) -> impl core::future::Future<Output = Result<Self, Self::Rejection>> {
        state.bodies.fetch_add(1, Ordering::SeqCst);
        core::future::ready(Ok(Self(body)))
    }
}

struct CombinedApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl CombinedApi {
    #[route(
        POST,
        "/combined",
        host = "api.example",
        consumes = "application/json",
        produces = "application/json"
    )]
    async fn combined(&self, probe: ProbeParts, headers: &HeaderMap, #[body] body: ProbeBody, state: State<Arc<ProbeState>>) -> String {
        let _ = probe;
        state.handlers.fetch_add(1, Ordering::SeqCst);
        assert_eq!(headers[HOST], "api.example");
        String::from_utf8(body.0).expect("test body is UTF-8")
    }
}

#[tokio::test]
async fn predicates_short_circuit_in_order_before_parts_body_and_handler_work() {
    let state = Arc::new(ProbeState::default());
    let requests = [
        (
            request_with_headers(
                MethodUri::Post("/combined"),
                &[(HOST, "wrong.example"), (CONTENT_TYPE, "text/plain"), (ACCEPT, "text/plain")],
                Vec::new(),
            ),
            StatusCode::NOT_FOUND,
        ),
        (
            request_with_headers(
                MethodUri::Post("/combined"),
                &[(HOST, "api.example"), (CONTENT_TYPE, "text/plain"), (ACCEPT, "text/plain")],
                Vec::new(),
            ),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            request_with_headers(
                MethodUri::Post("/combined"),
                &[
                    (HOST, "api.example"),
                    (CONTENT_TYPE, "application/json"),
                    (ACCEPT, "application/json;q=0"),
                ],
                Vec::new(),
            ),
            StatusCode::NOT_ACCEPTABLE,
        ),
    ];
    for (request, expected) in requests {
        let response = CombinedApi.route(request, &state).await;
        assert_eq!(response.status(), expected);
        assert!(!response.headers().contains_key(CONTENT_TYPE));
    }
    assert_eq!(state.parts.load(Ordering::SeqCst), 0);
    assert_eq!(state.bodies.load(Ordering::SeqCst), 0);
    assert_eq!(state.handlers.load(Ordering::SeqCst), 0);

    let success = request_with_headers(
        MethodUri::Post("/combined"),
        &[
            (HOST, "api.example"),
            (CONTENT_TYPE, "Application/JSON; charset=utf-8"),
            (ACCEPT, "application/*"),
        ],
        b"body".to_vec(),
    );
    let response = CombinedApi.route(success, &state).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(state.parts.load(Ordering::SeqCst), 1);
    assert_eq!(state.bodies.load(Ordering::SeqCst), 1);
    assert_eq!(state.handlers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn extractor_rejections_do_not_receive_produced_metadata() {
    let state = Arc::new(ProbeState::default());
    state.reject_parts.store(true, Ordering::SeqCst);
    let request = request_with_headers(
        MethodUri::Post("/combined"),
        &[
            (HOST, "api.example"),
            (CONTENT_TYPE, "application/json"),
            (ACCEPT, "application/json"),
        ],
        Vec::new(),
    );
    let response = CombinedApi.route(request, &state).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!response.headers().contains_key(CONTENT_TYPE));
    assert_eq!(state.bodies.load(Ordering::SeqCst), 0);
    assert_eq!(state.handlers.load(Ordering::SeqCst), 0);
}

struct DynamicApi {
    calls: Cell<usize>,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "Cell records direct handler calls and router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl DynamicApi {
    #[route(dynamic, host = "plugins.example", consumes = "application/json", produces = "application/json")]
    async fn plugin(&self, #[capture] name: String, #[body] body: RawBody<Vec<u8>>) -> String {
        self.calls.set(self.calls.get() + 1);
        format!("{name}:{}", String::from_utf8(body.into_inner()).expect("test body is UTF-8"))
    }
}

#[tokio::test]
async fn configured_dynamic_handlers_use_the_same_predicate_contract() {
    let router = DynamicApi::router_builder()
        .add_plugin("POST", "/plugins/{name}")
        .build()
        .expect("dynamic route is valid");
    let api = DynamicApi { calls: Cell::new(0) };

    let rejected = request_with_headers(
        MethodUri::Post("/plugins/tracing"),
        &[
            (HOST, "plugins.example"),
            (CONTENT_TYPE, "application/json"),
            (ACCEPT, "text/plain"),
        ],
        Vec::new(),
    );
    assert_eq!(router.route(&api, rejected, &()).await.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(api.calls.get(), 0);

    let accepted = request_with_headers(
        MethodUri::Post("/plugins/tracing"),
        &[
            (HOST, "PLUGINS.EXAMPLE"),
            (CONTENT_TYPE, "application/json"),
            (ACCEPT, "application/json"),
        ],
        b"enabled".to_vec(),
    );
    let response = router.route(&api, accepted, &()).await;
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(body_bytes(response.into_body()).await, b"tracing:enabled"[..]);
    assert_eq!(api.calls.get(), 1);
}

struct PreparedApi {
    response: RefCell<Option<Response<Body>>>,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "the prepared response isolates dispatch allocation accounting; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PreparedApi {
    #[route(
        GET,
        "/prepared",
        host = "api.example",
        consumes = "application/json",
        produces = "application/json"
    )]
    async fn prepared(&self) -> Response<Body> {
        self.response.borrow_mut().take().expect("one successful prepared dispatch")
    }
}

#[test]
#[cfg(not(miri))]
fn prepared_successful_and_rejected_predicate_dispatches_allocate_zero() {
    let prepared_response = Response::builder()
        .header(CONTENT_TYPE, "text/plain")
        .body(Body::empty())
        .expect("static response metadata is valid");
    let api = PreparedApi {
        response: RefCell::new(Some(prepared_response)),
    };
    let request = request_with_headers(
        MethodUri::Get("/prepared"),
        &[
            (HOST, "API.EXAMPLE"),
            (CONTENT_TYPE, "Application/JSON; charset=utf-8"),
            (ACCEPT, "*/*"),
        ],
        (),
    );
    let mut future = pin!(api.route(request, &()));
    let mut context = Context::from_waker(Waker::noop());
    let session = Session::new().no_stdout().no_file();
    let success = session.operation("prepared_predicate_success");
    let response = {
        let _span = success.measure_thread().iterations(1);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(response) => std::hint::black_box(response),
            Poll::Pending => panic!("the prepared predicate success has no pending operation"),
        }
    };
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(total_bytes_allocated(&session, "prepared_predicate_success"), 0);

    for (name, host, content_type, accept, expected) in [
        (
            "prepared_host_rejection",
            "wrong.example",
            "application/json",
            "application/json",
            StatusCode::NOT_FOUND,
        ),
        (
            "prepared_consumes_rejection",
            "api.example",
            "text/plain",
            "application/json",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "prepared_produces_rejection",
            "api.example",
            "application/json",
            "application/json;q=0",
            StatusCode::NOT_ACCEPTABLE,
        ),
    ] {
        let rejected_api = PreparedApi {
            response: RefCell::new(None),
        };
        let rejected = request_with_headers(
            MethodUri::Get("/prepared"),
            &[(HOST, host), (CONTENT_TYPE, content_type), (ACCEPT, accept)],
            (),
        );
        let mut future = pin!(rejected_api.route(rejected, &()));
        let rejection = session.operation(name);
        let response = {
            let _span = rejection.measure_thread().iterations(1);
            match future.as_mut().poll(&mut context) {
                Poll::Ready(response) => std::hint::black_box(response),
                Poll::Pending => panic!("the prepared predicate rejection has no pending operation"),
            }
        };
        assert_eq!(response.status(), expected);
        assert_eq!(total_bytes_allocated(&session, name), 0);
    }
}

#[derive(Clone, Copy)]
enum MethodUri<'a> {
    Get(&'a str),
    Post(&'a str),
}

fn request_with_headers<B>(method_uri: MethodUri<'_>, headers: &[(HeaderName, &'static str)], body: B) -> Request<B> {
    let (method, uri) = match method_uri {
        MethodUri::Get(uri) => (http::Method::GET, uri),
        MethodUri::Post(uri) => (http::Method::POST, uri),
    };
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(name.clone(), *value);
    }
    request.body(body).expect("test request metadata is valid")
}

async fn body_bytes<B>(body: B) -> Bytes
where
    B: HttpBody<Data = Bytes>,
    B::Error: fmt::Debug,
{
    body.collect().await.expect("response body succeeds").to_bytes()
}

#[derive(Debug)]
struct StreamError;

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("test stream error")
    }
}

impl std::error::Error for StreamError {}

struct TrailerBody {
    state: u8,
}

impl TrailerBody {
    const fn new() -> Self {
        Self { state: 0 }
    }
}

impl HttpBody for TrailerBody {
    type Data = Bytes;
    type Error = StreamError;

    fn poll_frame(mut self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let frame = match self.state {
            0 => Some(Frame::data(Bytes::from_static(b"stream"))),
            1 => {
                let mut trailers = HeaderMap::new();
                trailers.insert("x-stream-complete", HeaderValue::from_static("yes"));
                Some(Frame::trailers(trailers))
            }
            _ => None,
        };
        self.state = self.state.saturating_add(1);
        Poll::Ready(frame.map(Ok))
    }

    fn is_end_stream(&self) -> bool {
        self.state > 1
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
