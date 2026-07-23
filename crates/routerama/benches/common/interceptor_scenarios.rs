// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared generated-interceptor scenarios. Each subgroup pairs an
// interceptor-free control against the same request handled by the same route
// with interceptors attached, so the reported difference is the interceptor's
// own cost and nothing else.
//
// `before` and `after` interceptors are deliberately passive: every scenario in
// those two subgroups returns a byte-identical response, which the equivalence
// test asserts. `transform` compares ordinary bounded `#[body]` extraction
// against the same extraction behind a buffering and a streaming transform.
//
// Services and requests are built in `prepare`; `run_prepared` contains exactly
// the routing call and the complete response observation.

use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::{Request, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use routerama::response::{Body, Response};
use routerama::route::{
    AfterContext, Before, BeforeContext, BodyTransform, BytesBody, RequestParts, router,
};

const PAYLOAD: &[u8] = b"interceptor-fixture-payload";
const REPLY: &[u8] = b"ok";

/// A one-frame transport body. It is `Unpin`, so the streaming transform's
/// wrapper needs no projection machinery on the measured path.
struct StaticBody {
    remaining: Option<Bytes>,
}

impl StaticBody {
    const fn new() -> Self {
        Self {
            remaining: Some(Bytes::from_static(PAYLOAD)),
        }
    }
}

impl HttpBody for StaticBody {
    type Data = Bytes;
    type Error = core::convert::Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.remaining.take().map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.remaining.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining.as_ref().map_or(0, Bytes::len) as u64)
    }
}

/// The streaming transform's replacement body: it counts data bytes as they
/// pass and never buffers.
struct Counted<B> {
    inner: B,
    seen: usize,
}

impl<B> HttpBody for Counted<B>
where
    B: HttpBody<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let polled = Pin::new(&mut self.inner).poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let Some(data) = frame.data_ref()
        {
            self.seen += data.len();
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn reply() -> (StatusCode, Bytes) {
    (StatusCode::OK, Bytes::from_static(REPLY))
}

/// The interceptor-free control for both the `before` and `after` subgroups.
struct BareApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl BareApi {
    #[route(GET, "/probe")]
    async fn probe(&self) -> (StatusCode, Bytes) {
        reply()
    }
}

/// One passive router-wide `#[before]`.
struct OneBeforeApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::needless_pass_by_ref_mut,
    reason = "router handlers and interceptors must be async and take the macro-required &mut context; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl OneBeforeApi {
    #[route(GET, "/probe")]
    async fn probe(&self) -> (StatusCode, Bytes) {
        reply()
    }

    #[before]
    async fn first(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _ = std::hint::black_box(ctx.method());
        Before::Next
    }
}

/// Four passive router-wide `#[before]` interceptors.
struct FourBeforeApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::needless_pass_by_ref_mut,
    reason = "router handlers and interceptors must be async and take the macro-required &mut context; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl FourBeforeApi {
    #[route(GET, "/probe")]
    async fn probe(&self) -> (StatusCode, Bytes) {
        reply()
    }

    #[before]
    async fn first(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _ = std::hint::black_box(ctx.method());
        Before::Next
    }

    #[before]
    async fn second(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _ = std::hint::black_box(ctx.method());
        Before::Next
    }

    #[before]
    async fn third(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _ = std::hint::black_box(ctx.method());
        Before::Next
    }

    #[before]
    async fn fourth(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _ = std::hint::black_box(ctx.method());
        Before::Next
    }
}

/// One passive `#[after]`.
struct OneAfterApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::needless_pass_by_ref_mut,
    reason = "router handlers and interceptors must be async and take the macro-required &mut context; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl OneAfterApi {
    #[route(GET, "/probe")]
    async fn probe(&self) -> (StatusCode, Bytes) {
        reply()
    }

    #[after]
    async fn first(&self, ctx: &mut AfterContext<'_>) {
        let _ = std::hint::black_box(ctx.status());
    }
}

/// Four passive `#[after]` interceptors.
struct FourAfterApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::needless_pass_by_ref_mut,
    reason = "router handlers and interceptors must be async and take the macro-required &mut context; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl FourAfterApi {
    #[route(GET, "/probe")]
    async fn probe(&self) -> (StatusCode, Bytes) {
        reply()
    }

    #[after]
    async fn first(&self, ctx: &mut AfterContext<'_>) {
        let _ = std::hint::black_box(ctx.status());
    }

    #[after]
    async fn second(&self, ctx: &mut AfterContext<'_>) {
        let _ = std::hint::black_box(ctx.status());
    }

    #[after]
    async fn third(&self, ctx: &mut AfterContext<'_>) {
        let _ = std::hint::black_box(ctx.status());
    }

    #[after]
    async fn fourth(&self, ctx: &mut AfterContext<'_>) {
        let _ = std::hint::black_box(ctx.status());
    }
}

/// Ordinary bounded extraction, the same extraction behind a buffering
/// `#[transform(limit = N, ...)]`, and behind a streaming
/// `#[transform(stream, ...)]`.
struct TransformApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl TransformApi {
    #[route(POST, "/plain")]
    async fn plain(&self, #[body] body: BytesBody<64>) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(body.as_bytes().len());
        reply()
    }

    #[route(POST, "/bounded")]
    async fn bounded(&self, #[body] body: BytesBody<64>) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(body.as_bytes().len());
        reply()
    }

    #[route(POST, "/streamed")]
    async fn streamed(&self, #[body] body: BytesBody<64>) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(body.as_bytes().len());
        reply()
    }

    /// Buffers the named handler's body and hands back a concrete replacement
    /// that reuses the already-collected bytes.
    #[transform(limit = 64, bounded)]
    async fn buffer(&self, parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        let _ = std::hint::black_box(&parts.method);
        BodyTransform::Replace(Body::from_bytes(body))
    }

    /// Wraps the named handler's transport body lazily; nothing is buffered,
    /// boxed, or dynamically dispatched.
    #[transform(stream, streamed)]
    async fn count<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<Counted<B>, StatusCode>
    where
        B: HttpBody<Data = Bytes> + Unpin,
    {
        let _ = std::hint::black_box(&parts.method);
        BodyTransform::Replace(Counted { inner: body, seen: 0 })
    }
}

static BARE_API: BareApi = BareApi;
static ONE_BEFORE_API: OneBeforeApi = OneBeforeApi;
static FOUR_BEFORE_API: FourBeforeApi = FourBeforeApi;
static ONE_AFTER_API: OneAfterApi = OneAfterApi;
static FOUR_AFTER_API: FourAfterApi = FourAfterApi;
static TRANSFORM_API: TransformApi = TransformApi;

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
    body: Fingerprint,
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
            Poll::Ready(Some(Err(_))) => panic!("the interceptor evidence bodies never fail"),
            Poll::Ready(None) => break,
            Poll::Pending => panic!("the in-memory evidence bodies must always be ready"),
        }
    }
    Observation {
        status,
        body: fingerprint,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    BeforeNone,
    BeforeOne,
    BeforeFour,
    AfterNone,
    AfterOne,
    AfterFour,
    TransformNone,
    TransformBounded,
    TransformStreaming,
}

impl Scenario {
    const ALL: [Self; 9] = [
        Self::BeforeNone,
        Self::BeforeOne,
        Self::BeforeFour,
        Self::AfterNone,
        Self::AfterOne,
        Self::AfterFour,
        Self::TransformNone,
        Self::TransformBounded,
        Self::TransformStreaming,
    ];

    const fn group(self) -> &'static str {
        match self {
            Self::BeforeNone | Self::BeforeOne | Self::BeforeFour => "before",
            Self::AfterNone | Self::AfterOne | Self::AfterFour => "after",
            Self::TransformNone | Self::TransformBounded | Self::TransformStreaming => "transform",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::BeforeNone | Self::AfterNone | Self::TransformNone => "none",
            Self::BeforeOne | Self::AfterOne => "one",
            Self::BeforeFour | Self::AfterFour => "four",
            Self::TransformBounded => "bounded",
            Self::TransformStreaming => "streaming",
        }
    }

    fn diagnostic_name(self) -> String {
        format!("{}/{}", self.group(), self.name())
    }
}

enum PreparedScenario {
    Bare(Request<()>),
    OneBefore(Request<()>),
    FourBefore(Request<()>),
    OneAfter(Request<()>),
    FourAfter(Request<()>),
    Transform(Request<StaticBody>),
}

fn probe_request() -> Request<()> {
    Request::builder()
        .method("GET")
        .uri("/probe")
        .body(())
        .expect("the interceptor probe request metadata is valid")
}

fn body_request(path: &'static str) -> Request<StaticBody> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(http::header::CONTENT_LENGTH, PAYLOAD.len().to_string())
        .body(StaticBody::new())
        .expect("the interceptor body request metadata is valid")
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    match scenario {
        Scenario::BeforeNone | Scenario::AfterNone => PreparedScenario::Bare(probe_request()),
        Scenario::BeforeOne => PreparedScenario::OneBefore(probe_request()),
        Scenario::BeforeFour => PreparedScenario::FourBefore(probe_request()),
        Scenario::AfterOne => PreparedScenario::OneAfter(probe_request()),
        Scenario::AfterFour => PreparedScenario::FourAfter(probe_request()),
        Scenario::TransformNone => PreparedScenario::Transform(body_request("/plain")),
        Scenario::TransformBounded => PreparedScenario::Transform(body_request("/bounded")),
        Scenario::TransformStreaming => PreparedScenario::Transform(body_request("/streamed")),
    }
}

fn run_prepared(prepared: PreparedScenario) -> Observation {
    match std::hint::black_box(prepared) {
        PreparedScenario::Bare(request) => observe(run_ready(BARE_API.route(request, &()))),
        PreparedScenario::OneBefore(request) => observe(run_ready(ONE_BEFORE_API.route(request, &()))),
        PreparedScenario::FourBefore(request) => observe(run_ready(FOUR_BEFORE_API.route(request, &()))),
        PreparedScenario::OneAfter(request) => observe(run_ready(ONE_AFTER_API.route(request, &()))),
        PreparedScenario::FourAfter(request) => observe(run_ready(FOUR_AFTER_API.route(request, &()))),
        PreparedScenario::Transform(request) => observe(run_ready(TRANSFORM_API.route(request, &()))),
    }
}

fn expected() -> Observation {
    Observation {
        status: 200,
        body: Fingerprint::of(REPLY),
    }
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        assert_eq!(
            run_prepared(prepare(scenario)),
            expected(),
            "{} changed its routed response; every interceptor scenario must stay response-identical",
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

fn allocation_diagnostics() -> [(Scenario, AllocationStats); 9] {
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
