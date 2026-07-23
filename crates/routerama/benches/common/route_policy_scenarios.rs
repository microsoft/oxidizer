// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared route-policy scenarios. Every measured call is one prepared
// `http::Request` driven through one generated `route` entry to a complete
// response observation. Services, route tables, and requests are built in
// `prepare`; `run_prepared` contains exactly the routing call and the response
// observation.
//
// These are Routerama-internal controls: each subgroup pairs a policy-bearing
// path against the plainest generated path that reaches the same boundary, so
// the published number is the cost of the policy, not of routing in general.
// They are deliberately not comparable to the five-framework fixtures.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Request, StatusCode};
use http_body::Body as HttpBody;
use routerama::response::{Body, IntoResponse, Response};
use routerama::route::{FromRequestParts, RequestParts, RouteFailure, router};

#[derive(Clone, Copy, Debug)]
struct MissingTrace;

impl IntoResponse for MissingTrace {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        StatusCode::BAD_REQUEST.into_response()
    }
}

/// A parts extractor that rejects unless the request carries `x-trace`.
struct RequiredTrace;

impl<S: ?Sized> FromRequestParts<'_, S> for RequiredTrace {
    type Rejection = MissingTrace;

    fn from_request_parts(parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.headers.contains_key("x-trace").then_some(Self).ok_or(MissingTrace)
    }
}

/// Overlapping candidates ranked by `priority`, plus one unambiguous control
/// route with the same handler shape and the same capture coercion.
struct PriorityApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PriorityApi {
    #[route(GET, "/reports/{id}", produces = "application/json", priority = 10)]
    async fn json(&self, id: u32) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(id);
        (StatusCode::OK, Bytes::from_static(b"json"))
    }

    #[route(GET, "/reports/{id}", produces = "text/plain", priority = 0)]
    async fn text(&self, id: u32) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(id);
        (StatusCode::OK, Bytes::from_static(b"text"))
    }

    #[route(GET, "/plain/{id}")]
    async fn plain(&self, id: u32) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(id);
        (StatusCode::OK, Bytes::from_static(b"plain"))
    }
}

/// Host, `consumes`, and `produces` predicates, plus one predicate-free control
/// route reached through the same generated entry.
struct PredicateApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PredicateApi {
    #[route(
        POST,
        "/negotiated",
        host = "api.example",
        consumes = "application/json",
        produces = "application/json"
    )]
    async fn negotiated(&self) -> (StatusCode, Bytes) {
        (StatusCode::OK, Bytes::from_static(b"negotiated"))
    }

    #[route(POST, "/unconstrained")]
    async fn unconstrained(&self) -> (StatusCode, Bytes) {
        (StatusCode::OK, Bytes::from_static(b"unconstrained"))
    }
}

/// The control service: no `#[fallback]`, no `#[catch]`. A miss takes the
/// generated default 404 and an extractor rejection takes the rejection's own
/// `IntoResponse`.
struct DefaultPolicyApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl DefaultPolicyApi {
    #[route(GET, "/secure")]
    async fn secure(&self, trace: RequiredTrace) -> (StatusCode, Bytes) {
        let _ = trace;
        (StatusCode::OK, Bytes::from_static(b"secure"))
    }
}

/// The same route table with a typed routing fallback and a typed extractor
/// catcher attached.
struct TypedPolicyApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers and policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl TypedPolicyApi {
    #[route(GET, "/secure")]
    async fn secure(&self, trace: RequiredTrace) -> (StatusCode, Bytes) {
        let _ = trace;
        (StatusCode::OK, Bytes::from_static(b"secure"))
    }

    #[catch(MissingTrace, from = RequiredTrace)]
    async fn catch_trace(&self, rejection: MissingTrace) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(rejection);
        (StatusCode::UNAUTHORIZED, Bytes::from_static(b"caught"))
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> (StatusCode, Bytes) {
        (failure.status(), Bytes::from_static(b"fallback"))
    }
}

static PRIORITY_API: PriorityApi = PriorityApi;
static PREDICATE_API: PredicateApi = PredicateApi;
static DEFAULT_POLICY_API: DefaultPolicyApi = DefaultPolicyApi;
static TYPED_POLICY_API: TypedPolicyApi = TypedPolicyApi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    length: usize,
    hash: u64,
}

impl Fingerprint {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn empty() -> Self {
        Self {
            length: 0,
            hash: Self::OFFSET,
        }
    }

    fn of(bytes: &[u8]) -> Self {
        let mut fingerprint = Self::empty();
        fingerprint.push(bytes);
        fingerprint
    }

    fn push(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(Self::PRIME);
        }
        self.length += bytes.len();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    status: u16,
    content_type: Option<Fingerprint>,
    body: Fingerprint,
}

impl Observation {
    fn new(status: u16, content_type: Option<&str>, body: &str) -> Self {
        Self {
            status,
            content_type: content_type.map(|value| Fingerprint::of(value.as_bytes())),
            body: Fingerprint::of(body.as_bytes()),
        }
    }
}

fn content_type(headers: &HeaderMap) -> Option<Fingerprint> {
    headers
        .get(http::header::CONTENT_TYPE)
        .map(|value| Fingerprint::of(value.as_bytes()))
}

fn run_ready<F>(future: F) -> F::Output
where
    F: Future,
{
    // Stack-pin to avoid allocator noise on the measured route path.
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory generated route future must complete in one poll"),
    }
}

fn observe<B>(response: Response<B>) -> Observation
where
    B: HttpBody<Data = Bytes>,
{
    let status = response.status().as_u16();
    let content_type = content_type(response.headers());
    // Stack-pin to keep body polling allocation-free on the measured path.
    let mut body = pin!(response.into_body());
    let mut context = Context::from_waker(Waker::noop());
    let mut fingerprint = Fingerprint::empty();
    loop {
        match body.as_mut().poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    fingerprint.push(data);
                }
            }
            Poll::Ready(Some(Err(_))) => panic!("the route-policy evidence bodies never fail"),
            Poll::Ready(None) => break,
            Poll::Pending => panic!("the in-memory evidence bodies must always be ready"),
        }
    }
    Observation {
        status,
        content_type,
        body: fingerprint,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    PriorityPlain,
    PriorityHighestCandidate,
    PriorityLowerCandidate,
    PredicateUnconstrained,
    PredicateAccepted,
    PredicateUnsupportedMediaType,
    PredicateNotAcceptable,
    FallbackDefaultMiss,
    FallbackTypedMiss,
    CatcherDefaultRejection,
    CatcherTypedRejection,
}

impl Scenario {
    const ALL: [Self; 11] = [
        Self::PriorityPlain,
        Self::PriorityHighestCandidate,
        Self::PriorityLowerCandidate,
        Self::PredicateUnconstrained,
        Self::PredicateAccepted,
        Self::PredicateUnsupportedMediaType,
        Self::PredicateNotAcceptable,
        Self::FallbackDefaultMiss,
        Self::FallbackTypedMiss,
        Self::CatcherDefaultRejection,
        Self::CatcherTypedRejection,
    ];

    const fn group(self) -> &'static str {
        match self {
            Self::PriorityPlain | Self::PriorityHighestCandidate | Self::PriorityLowerCandidate => "priority",
            Self::PredicateUnconstrained
            | Self::PredicateAccepted
            | Self::PredicateUnsupportedMediaType
            | Self::PredicateNotAcceptable => "predicates",
            Self::FallbackDefaultMiss | Self::FallbackTypedMiss => "fallback",
            Self::CatcherDefaultRejection | Self::CatcherTypedRejection => "catcher",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::PriorityPlain => "plain_route",
            Self::PriorityHighestCandidate => "highest_candidate",
            Self::PriorityLowerCandidate => "lower_candidate",
            Self::PredicateUnconstrained => "unconstrained",
            Self::PredicateAccepted => "accepted",
            Self::PredicateUnsupportedMediaType => "unsupported_media_type",
            Self::PredicateNotAcceptable => "not_acceptable",
            Self::FallbackDefaultMiss => "default_miss",
            Self::FallbackTypedMiss => "typed_miss",
            Self::CatcherDefaultRejection => "default_rejection",
            Self::CatcherTypedRejection => "typed_rejection",
        }
    }

    fn diagnostic_name(self) -> String {
        format!("{}/{}", self.group(), self.name())
    }

    fn expected(self) -> Observation {
        match self {
            Self::PriorityPlain => Observation::new(200, None, "plain"),
            Self::PriorityHighestCandidate => Observation::new(200, Some("application/json"), "json"),
            Self::PriorityLowerCandidate => Observation::new(200, Some("text/plain"), "text"),
            Self::PredicateUnconstrained => Observation::new(200, None, "unconstrained"),
            Self::PredicateAccepted => Observation::new(200, Some("application/json"), "negotiated"),
            Self::PredicateUnsupportedMediaType => Observation::new(415, None, ""),
            Self::PredicateNotAcceptable => Observation::new(406, None, ""),
            Self::FallbackDefaultMiss => Observation::new(404, None, ""),
            Self::FallbackTypedMiss => Observation::new(404, None, "fallback"),
            Self::CatcherDefaultRejection => Observation::new(400, None, ""),
            Self::CatcherTypedRejection => Observation::new(401, None, "caught"),
        }
    }
}

enum PreparedScenario {
    Priority(Request<()>),
    Predicate(Request<()>),
    DefaultPolicy(Request<()>),
    TypedPolicy(Request<()>),
}

fn request(method: &'static str, path: &'static str, headers: &[(&'static str, &'static str)]) -> Request<()> {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, HeaderValue::from_static(value));
    }
    builder.body(()).expect("the route-policy request metadata is valid")
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    match scenario {
        Scenario::PriorityPlain => PreparedScenario::Priority(request("GET", "/plain/42", &[])),
        Scenario::PriorityHighestCandidate => PreparedScenario::Priority(request(
            "GET",
            "/reports/42",
            &[("accept", "application/json")],
        )),
        Scenario::PriorityLowerCandidate => {
            PreparedScenario::Priority(request("GET", "/reports/42", &[("accept", "text/plain")]))
        }
        Scenario::PredicateUnconstrained => PreparedScenario::Predicate(request("POST", "/unconstrained", &[])),
        Scenario::PredicateAccepted => PreparedScenario::Predicate(request(
            "POST",
            "/negotiated",
            &[
                ("host", "api.example"),
                ("content-type", "application/json"),
                ("accept", "application/json"),
            ],
        )),
        Scenario::PredicateUnsupportedMediaType => PreparedScenario::Predicate(request(
            "POST",
            "/negotiated",
            &[
                ("host", "api.example"),
                ("content-type", "text/plain"),
                ("accept", "application/json"),
            ],
        )),
        Scenario::PredicateNotAcceptable => PreparedScenario::Predicate(request(
            "POST",
            "/negotiated",
            &[
                ("host", "api.example"),
                ("content-type", "application/json"),
                ("accept", "text/plain"),
            ],
        )),
        Scenario::FallbackDefaultMiss => PreparedScenario::DefaultPolicy(request("GET", "/absent", &[])),
        Scenario::FallbackTypedMiss => PreparedScenario::TypedPolicy(request("GET", "/absent", &[])),
        Scenario::CatcherDefaultRejection => PreparedScenario::DefaultPolicy(request("GET", "/secure", &[])),
        Scenario::CatcherTypedRejection => PreparedScenario::TypedPolicy(request("GET", "/secure", &[])),
    }
}

fn run_prepared(prepared: PreparedScenario) -> Observation {
    match std::hint::black_box(prepared) {
        PreparedScenario::Priority(request) => observe(run_ready(PRIORITY_API.route(request, &()))),
        PreparedScenario::Predicate(request) => observe(run_ready(PREDICATE_API.route(request, &()))),
        PreparedScenario::DefaultPolicy(request) => observe(run_ready(DEFAULT_POLICY_API.route(request, &()))),
        PreparedScenario::TypedPolicy(request) => observe(run_ready(TYPED_POLICY_API.route(request, &()))),
    }
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        assert_eq!(
            run_prepared(prepare(scenario)),
            scenario.expected(),
            "{} changed its routed response",
            scenario.diagnostic_name()
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

fn report_stats(report: &alloc_tracker::Report, name: &str) -> AllocationStats {
    let (_, operation) = report
        .operations()
        .find(|(operation_name, _)| *operation_name == name)
        .expect("each allocation diagnostic records its named operation");
    AllocationStats {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
    }
}

fn allocation_diagnostics() -> [(Scenario, AllocationStats); 11] {
    // One unmeasured sweep first: the first routed request on a thread pays
    // one-time lazy initialization that is not part of the steady-state path.
    for scenario in Scenario::ALL {
        std::hint::black_box(run_prepared(prepare(scenario)));
    }

    Scenario::ALL.map(|scenario| {
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let prepared = std::hint::black_box(prepare(scenario));
        let operation = session.operation("measured");
        {
            let _span = operation.measure_thread();
            std::hint::black_box(run_prepared(prepared));
        }
        let report = session.to_report();
        (scenario, report_stats(&report, "measured"))
    })
}
