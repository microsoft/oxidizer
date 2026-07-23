// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared response-body representation scenarios. Request and concrete-body
// construction happen in `prepare`; `run_prepared` contains exactly the route,
// optional BoxBody boundary, frame polling, and observation named by each case.

use std::cell::Cell;
use std::fmt;
use std::mem::{size_of, size_of_val};
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, Uri};
use http_body::{Body as HttpBody, Frame, SizeHint};
use routerama::response::{
    Body, BoxBody, BoxBodyError, EitherBody, IntoResponse, IntoResponseParts, Response, ResponseParts,
};
use routerama::route::RawBody;

const FIXED_BYTES: &[u8] = b"fixed-body";
const STREAM_FIRST: &[u8] = b"first-frame";
const STREAM_SECOND: &[u8] = b"second-frame";
const TRAILER_NAME: HeaderName = HeaderName::from_static("x-stream-complete");
const TRAILER_VALUE: HeaderValue = HeaderValue::from_static("yes");
thread_local! {
    static NEXT_FAILURE_IDENTITY: Cell<u64> = const { Cell::new(1) };
    static LAST_DROPPED_FAILURE: Cell<Option<u64>> = const { Cell::new(None) };
}

fn next_failure_identity() -> u64 {
    NEXT_FAILURE_IDENTITY.with(|next| {
        let identity = next.get();
        next.set(
            identity
                .checked_add(1)
                .expect("a fixture thread cannot prepare u64::MAX failure bodies"),
        );
        identity
    })
}

fn last_dropped_failure() -> Option<u64> {
    LAST_DROPPED_FAILURE.get()
}

#[derive(Debug)]
struct FailureDropWitness {
    identity: u64,
    dropped: AtomicBool,
}

impl FailureDropWitness {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            identity: next_failure_identity(),
            dropped: AtomicBool::new(false),
        })
    }
}

fn assert_failure_is_owned(witness: &FailureDropWitness) {
    assert!(
        !witness.dropped.load(Ordering::Relaxed),
        "the routed error must still own the exact prepared failure instance"
    );
    assert_ne!(
        last_dropped_failure(),
        Some(witness.identity),
        "the routed error must still own the exact prepared failure instance"
    );
}

fn assert_failure_was_dropped(witness: &FailureDropWitness) {
    assert!(
        witness.dropped.load(Ordering::Relaxed),
        "dropping the routed error must drop the exact prepared failure instance"
    );
    assert_eq!(
        last_dropped_failure(),
        Some(witness.identity),
        "dropping the routed error must drop the exact prepared failure instance"
    );
}

#[derive(Debug)]
struct StreamFailure;

impl fmt::Display for StreamFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("stream failure")
    }
}

impl std::error::Error for StreamFailure {}

#[derive(Debug)]
struct ObservedFailure {
    witness: Arc<FailureDropWitness>,
}

impl fmt::Display for ObservedFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "observed failure {:#018x}", self.witness.identity)
    }
}

impl std::error::Error for ObservedFailure {}

impl Drop for ObservedFailure {
    fn drop(&mut self) {
        self.witness.dropped.store(true, Ordering::Relaxed);
        LAST_DROPPED_FAILURE.set(Some(self.witness.identity));
    }
}

struct StreamBody {
    next: u8,
    first: Bytes,
    second: Bytes,
    trailers: Option<HeaderMap>,
}

impl StreamBody {
    fn success() -> Self {
        let mut trailers = HeaderMap::new();
        trailers.insert(TRAILER_NAME, TRAILER_VALUE);
        Self {
            next: 0,
            first: Bytes::from_static(STREAM_FIRST),
            second: Bytes::from_static(STREAM_SECOND),
            trailers: Some(trailers),
        }
    }

    fn remaining_data_length(&self) -> usize {
        match self.next {
            0 => self.first.len() + self.second.len(),
            1 => self.second.len(),
            _ => 0,
        }
    }
}

impl HttpBody for StreamBody {
    type Data = Bytes;
    type Error = StreamFailure;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let _ = cx;
        let this = self.get_mut();
        let frame = match this.next {
            0 => {
                this.next = 1;
                Some(Ok(Frame::data(core::mem::take(&mut this.first))))
            }
            1 => {
                this.next = 2;
                Some(Ok(Frame::data(core::mem::take(&mut this.second))))
            }
            2 => {
                this.next = 3;
                this.trailers.take().map(|trailers| Ok(Frame::trailers(trailers)))
            }
            _ => None,
        };
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.next >= 3
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining_data_length() as u64)
    }
}

struct FailureBody {
    yielded: bool,
    witness: Arc<FailureDropWitness>,
}

impl FailureBody {
    fn new() -> Self {
        Self {
            yielded: false,
            witness: FailureDropWitness::new(),
        }
    }

    fn witness(&self) -> Arc<FailureDropWitness> {
        Arc::clone(&self.witness)
    }
}

impl HttpBody for FailureBody {
    type Data = Bytes;
    type Error = ObservedFailure;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let _ = cx;
        if self.yielded {
            return Poll::Ready(None);
        }
        self.yielded = true;
        Poll::Ready(Some(Err(ObservedFailure {
            witness: Arc::clone(&self.witness),
        })))
    }

    fn is_end_stream(&self) -> bool {
        self.yielded
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(0)
    }
}

enum ResponseInput {
    Unused,
    Stream(StreamBody),
    Failure(FailureBody),
}

impl ResponseInput {
    fn into_stream(self) -> StreamBody {
        match self {
            Self::Stream(body) => body,
            Self::Unused | Self::Failure(_) => panic!("the stream route must receive the prepared stream body"),
        }
    }

    fn into_failure(self) -> FailureBody {
        match self {
            Self::Failure(body) => body,
            Self::Unused | Self::Stream(_) => panic!("the failure route must receive the prepared failure body"),
        }
    }
}

struct FixedBodyService;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl FixedBodyService {
    #[route(GET, "/fixed")]
    async fn fixed(&self) -> Body {
        Body::from(Bytes::from_static(FIXED_BYTES))
    }
}

struct GeneratedBodyService;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl GeneratedBodyService {
    #[route(GET, "/fixed")]
    async fn fixed(&self) -> Body {
        Body::from(Bytes::from_static(FIXED_BYTES))
    }

    #[route(GET, "/stream")]
    async fn stream(&self, #[body] body: RawBody<ResponseInput>) -> Response<StreamBody> {
        Response::new(body.into_inner().into_stream())
    }

    #[route(GET, "/boxed")]
    async fn boxed(&self, #[body] body: RawBody<ResponseInput>) -> BoxBody {
        BoxBody::new(body.into_inner().into_stream())
    }

    #[route(GET, "/failure")]
    async fn failure(&self, #[body] body: RawBody<ResponseInput>) -> Response<FailureBody> {
        Response::new(body.into_inner().into_failure())
    }

    #[route(GET, "/boxed-failure")]
    async fn boxed_failure(&self, #[body] body: RawBody<ResponseInput>) -> BoxBody {
        BoxBody::new(body.into_inner().into_failure())
    }
}

struct SendBodyService;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl SendBodyService {
    #[route(GET, "/fixed")]
    async fn fixed(&self) -> Body {
        Body::from(Bytes::from_static(FIXED_BYTES))
    }

    #[route(GET, "/stream")]
    async fn stream(&self) -> Response<StreamBody> {
        Response::new(StreamBody::success())
    }
}

struct LocalBody {
    inner: StreamBody,
    _not_send: Rc<()>,
}

impl HttpBody for LocalBody {
    type Data = Bytes;
    type Error = StreamFailure;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

struct LocalBodyService;

#[expect(
    clippy::future_not_send,
    reason = "the core-route compatibility case deliberately retains Rc across an await"
)]
#[routerama::route::router]
impl LocalBodyService {
    #[route(GET, "/local")]
    async fn local(&self) -> Response<LocalBody> {
        let not_send = Rc::new(());
        core::future::ready(()).await;
        Response::new(LocalBody {
            inner: StreamBody::success(),
            _not_send: not_send,
        })
    }
}

static FIXED_BODY_SERVICE: FixedBodyService = FixedBodyService;
static GENERATED_BODY_SERVICE: GeneratedBodyService = GeneratedBodyService;
static SEND_BODY_SERVICE: SendBodyService = SendBodyService;
static LOCAL_BODY_SERVICE: LocalBodyService = LocalBodyService;

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

    fn push_length(&mut self, length: usize) {
        self.push(&(length as u64).to_le_bytes());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HintObservation {
    lower: u64,
    upper: Option<u64>,
}

impl HintObservation {
    fn of(hint: &SizeHint) -> Self {
        Self {
            lower: hint.lower(),
            upper: hint.upper(),
        }
    }

    const fn exact(length: u64) -> Self {
        Self {
            lower: length,
            upper: Some(length),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BodyObservation {
    initial_end_stream: bool,
    final_end_stream: bool,
    initial_size_hint: HintObservation,
    data_frames: usize,
    data_bytes: usize,
    data: Fingerprint,
    trailer_frames: usize,
    trailer_fields: usize,
    trailers: Fingerprint,
    frame_order: Fingerprint,
}

impl BodyObservation {
    fn new<B: HttpBody>(body: &B) -> Self {
        let initial_end_stream = body.is_end_stream();
        Self {
            initial_end_stream,
            final_end_stream: initial_end_stream,
            initial_size_hint: HintObservation::of(&body.size_hint()),
            data_frames: 0,
            data_bytes: 0,
            data: Fingerprint::empty(),
            trailer_frames: 0,
            trailer_fields: 0,
            trailers: Fingerprint::empty(),
            frame_order: Fingerprint::empty(),
        }
    }

    fn observe_data(&mut self, data: &Bytes) {
        self.data_frames += 1;
        self.data_bytes += data.len();
        self.data.push(b"D");
        self.data.push_length(data.len());
        self.data.push(data);
        self.frame_order.push(b"D");
    }

    fn observe_trailers(&mut self, trailers: &HeaderMap) {
        self.trailer_frames += 1;
        self.frame_order.push(b"T");
        self.trailers.push(b"T");
        self.trailers.push_length(trailers.len());
        for (name, value) in trailers {
            self.trailer_fields += 1;
            self.trailers.push_length(name.as_str().len());
            self.trailers.push(name.as_str().as_bytes());
            self.trailers.push_length(value.as_bytes().len());
            self.trailers.push(value.as_bytes());
        }
    }
}

enum BodyTerminal<E> {
    Complete(BodyObservation),
    Failed(BodyObservation, E),
}

fn poll_body<B>(body: B) -> BodyTerminal<B::Error>
where
    B: HttpBody<Data = Bytes>,
{
    // Stack-pin to keep body polling allocation-free on the measured path.
    let mut body = pin!(body);
    let mut observation = BodyObservation::new(body.as_ref().get_ref());
    let mut context = Context::from_waker(Waker::noop());

    loop {
        match body.as_mut().poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                Ok(data) => observation.observe_data(&data),
                Err(frame) => {
                    let trailers = frame
                        .into_trailers()
                        .expect("an http-body frame that is not data must contain trailers");
                    observation.observe_trailers(&trailers);
                }
            },
            Poll::Ready(Some(Err(error))) => {
                observation.final_end_stream = body.as_ref().is_end_stream();
                return BodyTerminal::Failed(observation, error);
            }
            Poll::Ready(None) => {
                observation.final_end_stream = body.as_ref().is_end_stream();
                return BodyTerminal::Complete(observation);
            }
            Poll::Pending => panic!("the in-memory evidence bodies must always be ready"),
        }
    }
}

fn observe_success<B>(body: B) -> BodyObservation
where
    B: HttpBody<Data = Bytes>,
    B::Error: fmt::Debug,
{
    match poll_body(body) {
        BodyTerminal::Complete(observation) => observation,
        BodyTerminal::Failed(_, error) => panic!("the successful evidence body failed: {error:?}"),
    }
}

fn failure_fingerprint(identity: u64) -> Fingerprint {
    let mut fingerprint = Fingerprint::empty();
    fingerprint.push(b"E");
    fingerprint.push(&identity.to_le_bytes());
    fingerprint
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ErrorObservation {
    body: BodyObservation,
    fingerprint: Fingerprint,
    identity: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScenarioObservation {
    Success(BodyObservation),
    Failure(ErrorObservation),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    DirectFixed,
    DirectConcreteStream,
    DirectBoxBody,
    GeneratedFixed,
    GeneratedConcreteStream,
    GeneratedBoxBody,
    GeneratedConcreteError,
    BoxedError,
}

impl Scenario {
    const ALL: [Self; 8] = [
        Self::DirectFixed,
        Self::DirectConcreteStream,
        Self::DirectBoxBody,
        Self::GeneratedFixed,
        Self::GeneratedConcreteStream,
        Self::GeneratedBoxBody,
        Self::GeneratedConcreteError,
        Self::BoxedError,
    ];

    const fn group(self) -> &'static str {
        match self {
            Self::DirectFixed | Self::DirectConcreteStream | Self::DirectBoxBody => "direct_observation",
            Self::GeneratedFixed | Self::GeneratedConcreteStream | Self::GeneratedBoxBody => "generated_route",
            Self::GeneratedConcreteError | Self::BoxedError => "error_propagation",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::DirectFixed | Self::GeneratedFixed => "fixed_body",
            Self::DirectConcreteStream | Self::GeneratedConcreteStream => "concrete_stream",
            Self::DirectBoxBody => "box_body_wrap_and_observe",
            Self::GeneratedBoxBody => "explicit_box_body",
            Self::GeneratedConcreteError => "generated_concrete",
            Self::BoxedError => "boxed",
        }
    }

    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::DirectFixed => "direct_observation/fixed_body",
            Self::DirectConcreteStream => "direct_observation/concrete_stream",
            Self::DirectBoxBody => "direct_observation/box_body_wrap_and_observe",
            Self::GeneratedFixed => "generated_route/fixed_body",
            Self::GeneratedConcreteStream => "generated_route/concrete_stream",
            Self::GeneratedBoxBody => "generated_route/explicit_box_body",
            Self::GeneratedConcreteError => "error_propagation/generated_concrete",
            Self::BoxedError => "error_propagation/boxed",
        }
    }

    const fn setup_operation(self) -> &'static str {
        match self {
            Self::DirectFixed => "direct_fixed_setup",
            Self::DirectConcreteStream => "direct_concrete_stream_setup",
            Self::DirectBoxBody => "direct_box_body_setup",
            Self::GeneratedFixed => "generated_fixed_setup",
            Self::GeneratedConcreteStream => "generated_concrete_stream_setup",
            Self::GeneratedBoxBody => "generated_box_body_setup",
            Self::GeneratedConcreteError => "generated_concrete_error_setup",
            Self::BoxedError => "boxed_error_setup",
        }
    }

    const fn measured_operation(self) -> &'static str {
        match self {
            Self::DirectFixed => "direct_fixed_measured",
            Self::DirectConcreteStream => "direct_concrete_stream_measured",
            Self::DirectBoxBody => "direct_box_body_measured",
            Self::GeneratedFixed => "generated_fixed_measured",
            Self::GeneratedConcreteStream => "generated_concrete_stream_measured",
            Self::GeneratedBoxBody => "generated_box_body_measured",
            Self::GeneratedConcreteError => "generated_concrete_error_measured",
            Self::BoxedError => "boxed_error_measured",
        }
    }
}

enum PreparedScenario {
    DirectFixed(Body),
    DirectConcreteStream(StreamBody),
    DirectBoxBody(StreamBody),
    GeneratedFixed(Request<()>),
    GeneratedConcreteStream(Request<ResponseInput>),
    GeneratedBoxBody(Request<ResponseInput>),
    GeneratedConcreteError(PreparedFailure),
    BoxedError(PreparedFailure),
}

struct PreparedFailure {
    request: Request<ResponseInput>,
    witness: Arc<FailureDropWitness>,
}

fn request<B>(path: &'static str, body: B) -> Request<B> {
    let mut request = Request::new(body);
    *request.uri_mut() = Uri::from_static(path);
    request
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    match scenario {
        Scenario::DirectFixed => PreparedScenario::DirectFixed(Body::from(Bytes::from_static(FIXED_BYTES))),
        Scenario::DirectConcreteStream => PreparedScenario::DirectConcreteStream(StreamBody::success()),
        Scenario::DirectBoxBody => PreparedScenario::DirectBoxBody(StreamBody::success()),
        Scenario::GeneratedFixed => PreparedScenario::GeneratedFixed(request("/fixed", ())),
        Scenario::GeneratedConcreteStream => {
            PreparedScenario::GeneratedConcreteStream(request("/stream", ResponseInput::Stream(StreamBody::success())))
        }
        Scenario::GeneratedBoxBody => {
            PreparedScenario::GeneratedBoxBody(request("/boxed", ResponseInput::Stream(StreamBody::success())))
        }
        Scenario::GeneratedConcreteError => {
            PreparedScenario::GeneratedConcreteError(prepare_failure("/failure"))
        }
        Scenario::BoxedError => PreparedScenario::BoxedError(prepare_failure("/boxed-failure")),
    }
}

fn prepare_failure(path: &'static str) -> PreparedFailure {
    let body = FailureBody::new();
    let witness = body.witness();
    PreparedFailure {
        request: request(path, ResponseInput::Failure(body)),
        witness,
    }
}

fn run_ready<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    // Stack-pin to avoid allocator noise on the measured route path.
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory generated route future must complete in one poll"),
    }
}

fn routed_error_observation(
    body: impl HttpBody<Data = Bytes, Error = impl std::error::Error>,
    witness: Arc<FailureDropWitness>,
) -> ErrorObservation {
    let BodyTerminal::Failed(observation, error) = poll_body(body) else {
        panic!("the generated failure route must yield its body error");
    };
    assert_failure_is_owned(&witness);
    std::hint::black_box(&error);
    let fingerprint = failure_fingerprint(witness.identity);
    drop(std::hint::black_box(error));
    assert_failure_was_dropped(&witness);
    let identity = witness.identity;
    drop(witness);
    ErrorObservation {
        body: observation,
        fingerprint,
        identity,
    }
}

fn run_prepared(prepared: PreparedScenario) -> ScenarioObservation {
    match std::hint::black_box(prepared) {
        PreparedScenario::DirectFixed(body) => ScenarioObservation::Success(observe_success(body)),
        PreparedScenario::DirectConcreteStream(body) => ScenarioObservation::Success(observe_success(body)),
        PreparedScenario::DirectBoxBody(body) => ScenarioObservation::Success(observe_success(BoxBody::new(body))),
        PreparedScenario::GeneratedFixed(request) => {
            let response = run_ready(FIXED_BODY_SERVICE.route(request, &()));
            ScenarioObservation::Success(observe_success(response.into_body()))
        }
        PreparedScenario::GeneratedConcreteStream(request) | PreparedScenario::GeneratedBoxBody(request) => {
            let response = run_ready(GENERATED_BODY_SERVICE.route(request, &()));
            ScenarioObservation::Success(observe_success(response.into_body()))
        }
        PreparedScenario::GeneratedConcreteError(prepared) | PreparedScenario::BoxedError(prepared) => {
            assert_failure_is_owned(&prepared.witness);
            let response = run_ready(GENERATED_BODY_SERVICE.route(prepared.request, &()));
            ScenarioObservation::Failure(routed_error_observation(response.into_body(), prepared.witness))
        }
    }
}

fn expected_fixed_observation() -> BodyObservation {
    let mut observation = BodyObservation {
        initial_end_stream: false,
        final_end_stream: true,
        initial_size_hint: HintObservation::exact(FIXED_BYTES.len() as u64),
        data_frames: 0,
        data_bytes: 0,
        data: Fingerprint::empty(),
        trailer_frames: 0,
        trailer_fields: 0,
        trailers: Fingerprint::empty(),
        frame_order: Fingerprint::empty(),
    };
    observation.observe_data(&Bytes::from_static(FIXED_BYTES));
    observation
}

fn expected_stream_observation() -> BodyObservation {
    let mut trailers = HeaderMap::new();
    trailers.insert(TRAILER_NAME, TRAILER_VALUE);
    let mut observation = BodyObservation {
        initial_end_stream: false,
        final_end_stream: true,
        initial_size_hint: HintObservation::exact((STREAM_FIRST.len() + STREAM_SECOND.len()) as u64),
        data_frames: 0,
        data_bytes: 0,
        data: Fingerprint::empty(),
        trailer_frames: 0,
        trailer_fields: 0,
        trailers: Fingerprint::empty(),
        frame_order: Fingerprint::empty(),
    };
    observation.observe_data(&Bytes::from_static(STREAM_FIRST));
    observation.observe_data(&Bytes::from_static(STREAM_SECOND));
    observation.observe_trailers(&trailers);
    observation
}

const fn expected_error_body_observation() -> BodyObservation {
    BodyObservation {
        initial_end_stream: false,
        final_end_stream: true,
        initial_size_hint: HintObservation::exact(0),
        data_frames: 0,
        data_bytes: 0,
        data: Fingerprint::empty(),
        trailer_frames: 0,
        trailer_fields: 0,
        trailers: Fingerprint::empty(),
        frame_order: Fingerprint::empty(),
    }
}

fn assert_equivalent() {
    let fixed = expected_fixed_observation();
    let stream = expected_stream_observation();
    let error_body = expected_error_body_observation();

    for scenario in Scenario::ALL {
        match (scenario, run_prepared(prepare(scenario))) {
            (
                Scenario::DirectFixed | Scenario::GeneratedFixed,
                ScenarioObservation::Success(observation),
            ) => assert_eq!(observation, fixed, "{} changed fixed-body behavior", scenario.diagnostic_name()),
            (
                Scenario::DirectConcreteStream
                | Scenario::DirectBoxBody
                | Scenario::GeneratedConcreteStream
                | Scenario::GeneratedBoxBody,
                ScenarioObservation::Success(observation),
            ) => assert_eq!(
                observation,
                stream,
                "{} changed a data frame, trailer, size hint, or stream state",
                scenario.diagnostic_name()
            ),
            (
                Scenario::GeneratedConcreteError | Scenario::BoxedError,
                ScenarioObservation::Failure(observation),
            ) => {
                assert_eq!(observation.body, error_body);
                assert_eq!(observation.fingerprint, failure_fingerprint(observation.identity));
            }
            (_, observation) => panic!("{} produced the wrong observation kind: {observation:?}", scenario.diagnostic_name()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationDiagnostic {
    scenario: Scenario,
    setup: AllocationStats,
    measured: AllocationStats,
}

fn report_stats(report: &alloc_tracker::Report, name: &str) -> AllocationStats {
    let (_, operation) = report
        .operations()
        .find(|(operation_name, _)| *operation_name == name)
        .expect("each allocation diagnostic records both named operations");
    AllocationStats {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
    }
}

fn allocation_diagnostics() -> [AllocationDiagnostic; 8] {
    Scenario::ALL.map(|scenario| {
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let setup_operation = session.operation(scenario.setup_operation());
        let prepared = {
            let _span = setup_operation.measure_thread();
            std::hint::black_box(prepare(scenario))
        };
        let measured_operation = session.operation(scenario.measured_operation());
        {
            let _span = measured_operation.measure_thread();
            std::hint::black_box(run_prepared(prepared));
        }
        let report = session.to_report();
        AllocationDiagnostic {
            scenario,
            setup: report_stats(&report, scenario.setup_operation()),
            measured: report_stats(&report, scenario.measured_operation()),
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SizeDiagnostics {
    body: usize,
    concrete_stream: usize,
    either_body: usize,
    box_body: usize,
    box_body_error: usize,
    fixed_service_future: usize,
    fixed_service_response: usize,
    fixed_service_opaque_body: usize,
    multiple_service_future: usize,
    multiple_service_response: usize,
    multiple_service_opaque_body: usize,
    generated_body_error_sum: usize,
}

fn route_value_sizes<B, S>(future: B) -> (usize, usize, usize)
where
    B: std::future::Future<Output = Response<S>>,
{
    let future_size = size_of_val(&future);
    let response = run_ready(future);
    let response_size = size_of_val(&response);
    let body = response.into_body();
    let body_size = size_of_val(&body);
    std::hint::black_box(body);
    (future_size, response_size, body_size)
}

fn generated_body_error_sum_size() -> usize {
    let prepared = prepare_failure("/failure");
    let response = run_ready(GENERATED_BODY_SERVICE.route(prepared.request, &()));
    let BodyTerminal::Failed(_, error) = poll_body(response.into_body()) else {
        panic!("the generated failure route must expose its error-sum value");
    };
    assert_failure_is_owned(&prepared.witness);
    let size = size_of_val(&error);
    drop(error);
    assert_failure_was_dropped(&prepared.witness);
    size
}

fn size_diagnostics() -> SizeDiagnostics {
    let fixed = route_value_sizes(FIXED_BODY_SERVICE.route(request("/fixed", ()), &()));
    let multiple = route_value_sizes(SEND_BODY_SERVICE.route(request("/fixed", ()), &()));
    SizeDiagnostics {
        body: size_of::<Body>(),
        concrete_stream: size_of::<StreamBody>(),
        either_body: size_of::<EitherBody<Body, StreamBody>>(),
        box_body: size_of::<BoxBody>(),
        box_body_error: size_of::<BoxBodyError>(),
        fixed_service_future: fixed.0,
        fixed_service_response: fixed.1,
        fixed_service_opaque_body: fixed.2,
        multiple_service_future: multiple.0,
        multiple_service_response: multiple.1,
        multiple_service_opaque_body: multiple.2,
        generated_body_error_sum: generated_body_error_sum_size(),
    }
}

fn core_transport_adapter<B>(response: Response<B>) -> BodyObservation
where
    B: HttpBody<Data = Bytes>,
    B::Error: std::error::Error,
{
    observe_success(response.into_body())
}

fn send_transport_adapter<B>(response: Response<B>) -> BodyObservation
where
    B: HttpBody<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    observe_success(response.into_body())
}

fn assert_transport_compatibility() {
    let send_response = run_ready(SEND_BODY_SERVICE.route(request("/stream", ()), &()));
    assert_eq!(send_transport_adapter(send_response), expected_stream_observation());

    let local_response = run_ready(LOCAL_BODY_SERVICE.route(request("/local", ()), &()));
    assert_eq!(core_transport_adapter(local_response), expected_stream_observation());
}

struct AllocationPart {
    reject: bool,
}

struct AllocationPartRejection;

impl IntoResponse for AllocationPartRejection {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Response::new(Body::from(Bytes::from_static(b"part-rejected")));
        *response.status_mut() = StatusCode::BAD_REQUEST;
        response
    }
}

impl IntoResponseParts for AllocationPart {
    type Error = AllocationPartRejection;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        if self.reject {
            Err(AllocationPartRejection)
        } else {
            *response.status_mut() = StatusCode::CREATED;
            Ok(response)
        }
    }
}

fn run_allocation_part(reject: bool) {
    let response = (AllocationPart { reject }, Body::from(Bytes::from_static(b"part-success"))).into_response();
    let status = response.status();
    let observation = observe_success(response.into_body());
    std::hint::black_box((status, observation));
}

fn response_part_allocation_diagnostics() -> [AllocationStats; 2] {
    [
        ("fallible_part_success", false),
        ("fallible_part_rejection", true),
    ]
    .map(|(name, reject)| {
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let operation = session.operation(name);
        {
            let _span = operation.measure_thread();
            run_allocation_part(std::hint::black_box(reject));
        }
        let report = session.to_report();
        report_stats(&report, name)
    })
}
