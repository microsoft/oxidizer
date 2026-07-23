// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Runtime-mount controls cover generated integration, erased dispatch, capture
// and depth spill boundaries, streaming errors, and table scaling. Setup is
// excluded. Allocation spans separate routing/response construction from body
// observation.

use std::cell::Cell;
use std::fmt::Write as _;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::StatusCode;
use http_body::{Body as HttpBody, Frame, SizeHint};
use routerama::response::{Body, Response};
use routerama::route::mount::{ErasedMountRouter, ErasedMountService, MountedRequest};
use routerama::route::{Request, router};

const SERVED: &[u8] = b"served";
const STREAM_FIRST: &[u8] = b"stream-first";
const STREAM_SECOND: &[u8] = b"stream-second";

/// The payload a failing mounted stream reports. It is deliberately not
/// zero-sized: boxing a zero-sized error would not allocate, and the boxed
/// error is one of the allocations this fixture names.
const FAILURE_CODE: u64 = 0xf0e1_d2c3_b4a5_9687;

/// The matcher keeps segment offsets for up to this many segments on the
/// stack. A table deeper than this and a request deeper than this together
/// force the documented heap scratch spill.
const INLINE_SEGMENTS: usize = 16;

/// Capture ranges up to this many stay inline in a mounted match.
const INLINE_CAPTURES: usize = 4;

#[derive(Clone)]
struct AppState {
    mounted_calls: Rc<Cell<usize>>,
}

fn served() -> (StatusCode, Bytes) {
    (StatusCode::OK, Bytes::from_static(SERVED))
}

/// A mounted body error whose payload is deliberately not zero-sized, so the
/// `BoxBody` error box it forces is a real allocation.
#[derive(Debug)]
struct StreamFailure {
    code: u64,
}

impl std::fmt::Display for StreamFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mounted stream failed with code {}", self.code)
    }
}

impl std::error::Error for StreamFailure {}

/// A concrete two-frame mounted response body that either completes or fails
/// on its second frame. Both frames are static, so streaming itself allocates
/// nothing.
struct MountStream {
    next: u8,
    fails: bool,
}

impl MountStream {
    const fn new(fails: bool) -> Self {
        Self { next: 0, fails }
    }
}

impl HttpBody for MountStream {
    type Data = Bytes;
    type Error = StreamFailure;

    fn poll_frame(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let frame = match this.next {
            0 => {
                this.next = 1;
                Some(Ok(Frame::data(Bytes::from_static(STREAM_FIRST))))
            }
            1 => {
                this.next = 2;
                if this.fails {
                    Some(Err(StreamFailure { code: FAILURE_CODE }))
                } else {
                    Some(Ok(Frame::data(Bytes::from_static(STREAM_SECOND))))
                }
            }
            _ => None,
        };
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.next >= 2
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

/// A generated static-only service that also opts into erased mounts, so the
/// identical route table can be reached with and without a mount table.
struct StaticApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl StaticApi {
    #[route(GET, "/static")]
    async fn statically(&self) -> (StatusCode, Bytes) {
        served()
    }
}

/// A generated service whose only route is registered at run time, so a
/// configured-dynamic hit and an erased mounted hit differ only in which side
/// of the same entry serves the request.
struct DynamicApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl DynamicApi {
    #[route(dynamic)]
    async fn configured(&self) -> (StatusCode, Bytes) {
        served()
    }
}

type Mounts = ErasedMountRouter<Body, AppState>;
type MountService = ErasedMountService<Body, AppState>;

/// How many mounted entries a generated scaling table holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableSize {
    Mounts16,
    Mounts128,
    Mounts1024,
}

impl TableSize {
    const ALL: [Self; 3] = [Self::Mounts16, Self::Mounts128, Self::Mounts1024];

    const fn entries(self) -> usize {
        match self {
            Self::Mounts16 => 16,
            Self::Mounts128 => 128,
            Self::Mounts1024 => 1024,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Mounts16 => "0016",
            Self::Mounts128 => "0128",
            Self::Mounts1024 => "1024",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Mounts16 => 0,
            Self::Mounts128 => 1,
            Self::Mounts1024 => 2,
        }
    }
}

/// Which entry of a scaling table a request selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Position {
    First,
    Middle,
    Last,
    Miss,
}

impl Position {
    const fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Middle => "middle",
            Self::Last => "last",
            Self::Miss => "miss",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Middle => 1,
            Self::Last => 2,
            Self::Miss => 3,
        }
    }
}

/// How many captures the mounted template declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureCount {
    None,
    One,
    Four,
    Five,
}

impl CaptureCount {
    const ALL: [Self; 4] = [Self::None, Self::One, Self::Four, Self::Five];

    const fn captures(self) -> usize {
        match self {
            Self::None => 0,
            Self::One => 1,
            Self::Four => 4,
            Self::Five => 5,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::One => "one",
            Self::Four => "four",
            Self::Five => "five",
        }
    }

    const fn template(self) -> &'static str {
        match self {
            Self::None => "/captures/none",
            Self::One => "/captures/one/{a}",
            Self::Four => "/captures/four/{a}/{b}/{c}/{d}",
            Self::Five => "/captures/five/{a}/{b}/{c}/{d}/{e}",
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::None => "/captures/none",
            Self::One => "/captures/one/11",
            Self::Four => "/captures/four/11/22/33/44",
            Self::Five => "/captures/five/11/22/33/44/55",
        }
    }

    /// Whether the mounted match must spill its capture ranges to the heap.
    const fn spills_capture_scratch(self) -> bool {
        self.captures() > INLINE_CAPTURES
    }
}

/// A generated scaling table plus the four request paths it is probed with.
struct ScaledTable {
    router: Mounts,
    paths: [String; 4],
}

impl ScaledTable {
    fn new(size: TableSize, service: &MountService) -> Self {
        let label = size.label();
        let paths: Vec<String> = (0..size.entries())
            .map(|entry| format!("/scale/mounts-{label}/mount-{entry:04}"))
            .collect();
        let mut builder = Mounts::builder();
        for path in &paths {
            builder = builder.mount("GET", path, service.clone());
        }
        let last = size.entries() - 1;
        Self {
            router: builder.build().expect("the generated mount scaling table is valid"),
            paths: [
                paths[0].clone(),
                paths[size.entries() / 2].clone(),
                paths[last].clone(),
                format!("/scale/mounts-{label}/mount-missing"),
            ],
        }
    }
}

/// Builds `/seg01/seg02/...` with exactly `segments` path segments.
fn depth_path(segments: usize) -> String {
    (1..=segments).fold(String::new(), |mut path, segment| {
        write!(path, "/seg{segment:02}").expect("writing into a String cannot fail");
        path
    })
}

struct MountFixture {
    state: AppState,
    /// The two-entry table shared by the generated wrapper rows and the
    /// `standalone` rows.
    mounts: Mounts,
    captures: Mounts,
    streams: Mounts,
    /// A table whose deepest template exceeds the inline segment boundary, so
    /// the fixed-size offset fast path is disabled for every request it serves.
    depth: Mounts,
    depth_paths: [String; 2],
    scaled: [ScaledTable; 3],
    dynamic: DynamicApiRouter,
}

impl MountFixture {
    fn new() -> Self {
        let state = AppState {
            mounted_calls: Rc::new(Cell::new(0)),
        };
        let served_service = MountService::from_async_fn(async |_request: MountedRequest<'_, Body>, state: &AppState| {
            state.mounted_calls.set(state.mounted_calls.get() + 1);
            served()
        });
        // Reads every capture the matched template declares, so a row's cost
        // includes materializing, reading, and converting exactly that many
        // capture ranges.
        let capture_service = MountService::from_async_fn(async |request: MountedRequest<'_, Body>, state: &AppState| {
            state.mounted_calls.set(state.mounted_calls.get() + 1);
            let mut checksum = 0_u32;
            for (_, value) in request.captures() {
                checksum = checksum.wrapping_add(value.parse::<u32>().unwrap_or_default());
            }
            let _ = std::hint::black_box(checksum);
            served()
        });
        let stream_service = |fails: bool| {
            MountService::from_async_fn(async move |_request: MountedRequest<'_, Body>, state: &AppState| {
                state.mounted_calls.set(state.mounted_calls.get() + 1);
                Response::new(MountStream::new(fails))
            })
        };

        let mut captures = Mounts::builder();
        for count in CaptureCount::ALL {
            captures = captures.mount("GET", count.template(), capture_service.clone());
        }

        let depth_paths = [depth_path(INLINE_SEGMENTS), depth_path(INLINE_SEGMENTS + 1)];
        let depth = Mounts::builder()
            .mount("GET", &depth_paths[0], served_service.clone())
            .mount("GET", &depth_paths[1], served_service.clone())
            .build()
            .expect("the benchmark depth mount table is valid");

        Self {
            mounts: Mounts::builder()
                .mount("GET", "/mounted", served_service.clone())
                .mount("GET", "/mounted/alias", served_service.clone())
                .build()
                .expect("the benchmark mount table is valid"),
            captures: captures.build().expect("the benchmark capture mount table is valid"),
            streams: Mounts::builder()
                .mount("GET", "/stream/success", stream_service(false))
                .mount("GET", "/stream/error", stream_service(true))
                .build()
                .expect("the benchmark streaming mount table is valid"),
            depth,
            depth_paths,
            scaled: TableSize::ALL.map(|size| ScaledTable::new(size, &served_service)),
            dynamic: DynamicApi::router_builder()
                .add_configured("GET", "/configured")
                .build()
                .expect("the configured dynamic route is valid"),
            state,
        }
    }
}

thread_local! {
    // Leaked so no measured request can drop the final owning reference and
    // perform teardown, and so the mount tables are never rebuilt per iteration.
    static FIXTURE: &'static MountFixture = Box::leak(Box::new(MountFixture::new()));
}

fn fixture() -> &'static MountFixture {
    FIXTURE.with(|fixture| *fixture)
}

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

    fn of(chunks: &[&[u8]]) -> Self {
        let mut fingerprint = Self::empty();
        for chunk in chunks {
            fingerprint.push(chunk);
        }
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
    /// Whether the response body ended with an error instead of end-of-stream.
    failed: bool,
}

impl Observation {
    fn served() -> Self {
        Self {
            status: 200,
            body: Fingerprint::of(&[SERVED]),
            failed: false,
        }
    }

    fn missing() -> Self {
        Self {
            status: 404,
            body: Fingerprint::empty(),
            failed: false,
        }
    }
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
    let mut failed = false;
    loop {
        match body.as_mut().poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    fingerprint.push(data);
                }
            }
            Poll::Ready(Some(Err(_))) => {
                failed = true;
                break;
            }
            Poll::Ready(None) => break,
            Poll::Pending => panic!("the in-memory evidence bodies must always be ready"),
        }
    }
    Observation {
        status,
        body: fingerprint,
        failed,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    StaticPlainRoute,
    StaticWithPopulatedMounts,
    ConfiguredDynamic,
    ErasedMount,
    StandaloneLiteral,
    StandaloneMiss,
    Captures(CaptureCount),
    StreamingSuccess,
    StreamingError,
    /// A request with exactly `INLINE_SEGMENTS` segments (inline offsets) or
    /// one more (heap offsets), through a table deep enough to disable the
    /// fixed-size fast path.
    Depth(bool),
    Table(TableSize, Position),
}

impl Scenario {
    const ALL: [Self; 26] = [
        Self::StaticPlainRoute,
        Self::StaticWithPopulatedMounts,
        Self::ConfiguredDynamic,
        Self::ErasedMount,
        Self::StandaloneLiteral,
        Self::StandaloneMiss,
        Self::Captures(CaptureCount::None),
        Self::Captures(CaptureCount::One),
        Self::Captures(CaptureCount::Four),
        Self::Captures(CaptureCount::Five),
        Self::StreamingSuccess,
        Self::StreamingError,
        Self::Depth(false),
        Self::Depth(true),
        Self::Table(TableSize::Mounts16, Position::First),
        Self::Table(TableSize::Mounts16, Position::Middle),
        Self::Table(TableSize::Mounts16, Position::Last),
        Self::Table(TableSize::Mounts16, Position::Miss),
        Self::Table(TableSize::Mounts128, Position::First),
        Self::Table(TableSize::Mounts128, Position::Middle),
        Self::Table(TableSize::Mounts128, Position::Last),
        Self::Table(TableSize::Mounts128, Position::Miss),
        Self::Table(TableSize::Mounts1024, Position::First),
        Self::Table(TableSize::Mounts1024, Position::Middle),
        Self::Table(TableSize::Mounts1024, Position::Last),
        Self::Table(TableSize::Mounts1024, Position::Miss),
    ];

    const SUBGROUPS: [&'static str; 7] = [
        "static_hit",
        "dynamic_dispatch",
        "standalone",
        "captures",
        "streaming",
        "depth",
        "table_size",
    ];

    const fn group(self) -> &'static str {
        match self {
            Self::StaticPlainRoute | Self::StaticWithPopulatedMounts => "static_hit",
            Self::ConfiguredDynamic | Self::ErasedMount => "dynamic_dispatch",
            Self::StandaloneLiteral | Self::StandaloneMiss => "standalone",
            Self::Captures(_) => "captures",
            Self::StreamingSuccess | Self::StreamingError => "streaming",
            Self::Depth(_) => "depth",
            Self::Table(_, _) => "table_size",
        }
    }

    fn name(self) -> String {
        match self {
            Self::StaticPlainRoute => "plain_route".to_owned(),
            Self::StaticWithPopulatedMounts => "populated_erased_mounts".to_owned(),
            Self::ConfiguredDynamic => "configured_dynamic".to_owned(),
            Self::ErasedMount => "erased_mount".to_owned(),
            Self::StandaloneLiteral => "literal".to_owned(),
            Self::StandaloneMiss => "complete_miss".to_owned(),
            Self::Captures(count) => count.name().to_owned(),
            Self::StreamingSuccess => "success".to_owned(),
            Self::StreamingError => "error".to_owned(),
            Self::Depth(spilled) => {
                let segments = if spilled { INLINE_SEGMENTS + 1 } else { INLINE_SEGMENTS };
                format!("segments_{segments}")
            }
            Self::Table(size, position) => format!("{}_{}", size.label(), position.name()),
        }
    }

    fn diagnostic_name(self) -> String {
        format!("{}/{}", self.group(), self.name())
    }

    /// The request path, borrowed from the process-lifetime fixture for the
    /// scenarios whose paths are generated at startup.
    fn path(self) -> &'static str {
        let fixture = fixture();
        match self {
            Self::StaticPlainRoute | Self::StaticWithPopulatedMounts => "/static",
            Self::ConfiguredDynamic => "/configured",
            Self::ErasedMount | Self::StandaloneLiteral => "/mounted",
            Self::StandaloneMiss => "/not-mounted",
            Self::Captures(count) => count.path(),
            Self::StreamingSuccess => "/stream/success",
            Self::StreamingError => "/stream/error",
            Self::Depth(spilled) => fixture.depth_paths[usize::from(spilled)].as_str(),
            Self::Table(size, position) => fixture.scaled[size.index()].paths[position.index()].as_str(),
        }
    }

    fn expected(self) -> Observation {
        match self {
            Self::StandaloneMiss | Self::Table(_, Position::Miss) => Observation::missing(),
            Self::StreamingSuccess => Observation {
                status: 200,
                body: Fingerprint::of(&[STREAM_FIRST, STREAM_SECOND]),
                failed: false,
            },
            Self::StreamingError => Observation {
                status: 200,
                body: Fingerprint::of(&[STREAM_FIRST]),
                failed: true,
            },
            _ => Observation::served(),
        }
    }

    /// Whether this scenario is expected to invoke an erased mounted service.
    const fn invokes_mounted_service(self) -> bool {
        match self {
            Self::StaticPlainRoute | Self::StaticWithPopulatedMounts | Self::ConfiguredDynamic | Self::StandaloneMiss => false,
            Self::Table(_, position) => !matches!(position, Position::Miss),
            _ => true,
        }
    }

    /// The named allocations this scenario is expected to perform.
    fn decomposition(self) -> Decomposition {
        Decomposition {
            // One `Box::pin` per erased service call.
            future: u64::from(self.invokes_mounted_service()),
            // One `BoxBody`: the mounted response body, or the fixed 404 body
            // a mount-table miss returns.
            body: match self {
                Self::StaticPlainRoute | Self::StaticWithPopulatedMounts | Self::ConfiguredDynamic => 0,
                _ => 1,
            },
            // One boxed `BoxBodyError`, only when a body actually fails.
            error: u64::from(matches!(self, Self::StreamingError)),
            // Matcher offset or capture-range scratch that leaves its inline
            // storage.
            scratch: match self {
                Self::Depth(spilled) => u64::from(spilled),
                Self::Captures(count) => u64::from(count.spills_capture_scratch()),
                _ => 0,
            },
        }
    }
}

/// Which named allocation each measured allocation is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Decomposition {
    /// Boxed erased-service futures.
    future: u64,
    /// Boxed response bodies.
    body: u64,
    /// Boxed response-body errors.
    error: u64,
    /// Matcher and capture scratch spills.
    scratch: u64,
}

impl Decomposition {
    /// Allocations expected inside the routing span.
    const fn routing(self) -> u64 {
        self.future + self.body + self.scratch
    }

    /// Allocations expected inside the body-observation span.
    const fn observing(self) -> u64 {
        self.error
    }
}

struct PreparedScenario {
    scenario: Scenario,
    request: Request<Body>,
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    // Touch the fixture so table construction never lands inside a measured
    // region, even on the very first prepared call.
    let path = scenario.path();
    PreparedScenario {
        scenario,
        request: Request::get(path)
            .body(Body::empty())
            .expect("the mount benchmark request metadata is valid"),
    }
}

/// Separates the routing span from the body-observation span.
///
/// The unmeasured implementation is what benchmarks run: it adds no branch and
/// no state, so the measured region is exactly the routing call plus the
/// response observation.
trait Phases {
    fn routing<T>(&self, run: impl FnOnce() -> T) -> T;
    fn observing<T>(&self, run: impl FnOnce() -> T) -> T;
}

struct Unmeasured;

impl Phases for Unmeasured {
    #[expect(clippy::inline_always, reason = "the measured region must not gain a call boundary")]
    #[inline(always)]
    fn routing<T>(&self, run: impl FnOnce() -> T) -> T {
        run()
    }

    #[expect(clippy::inline_always, reason = "the measured region must not gain a call boundary")]
    #[inline(always)]
    fn observing<T>(&self, run: impl FnOnce() -> T) -> T {
        run()
    }
}

struct Measured {
    routing: alloc_tracker::Operation,
    observing: alloc_tracker::Operation,
}

impl Phases for Measured {
    fn routing<T>(&self, run: impl FnOnce() -> T) -> T {
        let _span = self.routing.measure_thread().iterations(1);
        run()
    }

    fn observing<T>(&self, run: impl FnOnce() -> T) -> T {
        let _span = self.observing.measure_thread().iterations(1);
        run()
    }
}

fn drive<P: Phases>(prepared: PreparedScenario, phases: &P) -> Observation {
    let PreparedScenario { scenario, request } = std::hint::black_box(prepared);
    let fixture = fixture();
    let state = &fixture.state;
    match scenario {
        Scenario::StaticPlainRoute => {
            let response = phases.routing(|| run_ready(StaticApi.route(request, state)));
            phases.observing(|| observe(response))
        }
        Scenario::StaticWithPopulatedMounts => {
            let response = phases.routing(|| run_ready(StaticApi.route_with_erased_mounts(request, state, &fixture.mounts)));
            phases.observing(|| observe(response))
        }
        Scenario::ConfiguredDynamic | Scenario::ErasedMount => {
            let response = phases.routing(|| {
                run_ready(
                    fixture
                        .dynamic
                        .route_with_erased_mounts(&DynamicApi, request, state, &fixture.mounts),
                )
            });
            phases.observing(|| observe(response))
        }
        Scenario::StandaloneLiteral | Scenario::StandaloneMiss => {
            let response = phases.routing(|| run_ready(fixture.mounts.route(request, state)));
            phases.observing(|| observe(response))
        }
        Scenario::Captures(_) => {
            let response = phases.routing(|| run_ready(fixture.captures.route(request, state)));
            phases.observing(|| observe(response))
        }
        Scenario::StreamingSuccess | Scenario::StreamingError => {
            let response = phases.routing(|| run_ready(fixture.streams.route(request, state)));
            phases.observing(|| observe(response))
        }
        Scenario::Depth(_) => {
            let response = phases.routing(|| run_ready(fixture.depth.route(request, state)));
            phases.observing(|| observe(response))
        }
        Scenario::Table(size, _) => {
            let router = &fixture.scaled[size.index()].router;
            let response = phases.routing(|| run_ready(router.route(request, state)));
            phases.observing(|| observe(response))
        }
    }
}

fn run_prepared(prepared: PreparedScenario) -> Observation {
    drive(prepared, &Unmeasured)
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

/// How many erased mounted services each scenario invoked.
fn mounted_call_counts() -> Vec<(Scenario, usize)> {
    Scenario::ALL
        .into_iter()
        .map(|scenario| {
            let before = fixture().state.mounted_calls.get();
            let _ = std::hint::black_box(run_prepared(prepare(scenario)));
            (scenario, fixture().state.mounted_calls.get() - before)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhasedStats {
    routing: AllocationStats,
    observing: AllocationStats,
}

impl PhasedStats {
    const fn allocations(self) -> u64 {
        self.routing.allocations + self.observing.allocations
    }

    const fn bytes(self) -> u64 {
        self.routing.bytes + self.observing.bytes
    }
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

fn allocation_diagnostics() -> Vec<(Scenario, PhasedStats)> {
    // One unmeasured sweep first: the first routed request on a thread pays
    // one-time lazy initialization that is not part of the steady-state path.
    for scenario in Scenario::ALL {
        let _ = std::hint::black_box(run_prepared(prepare(scenario)));
    }

    Scenario::ALL
        .into_iter()
        .map(|scenario| {
            let session = alloc_tracker::Session::new().no_stdout().no_file();
            let prepared = std::hint::black_box(prepare(scenario));
            let phases = Measured {
                routing: session.operation("routing"),
                observing: session.operation("observing"),
            };
            let _ = std::hint::black_box(drive(prepared, &phases));
            let report = session.to_report();
            (
                scenario,
                PhasedStats {
                    routing: report_stats(&report, "routing"),
                    observing: report_stats(&report, "observing"),
                },
            )
        })
        .collect()
}

/// Checks the named allocation decomposition against the measured spans.
fn assert_allocation_decomposition() {
    for (scenario, stats) in allocation_diagnostics() {
        let expected = scenario.decomposition();
        assert_eq!(
            stats.routing.allocations,
            expected.routing(),
            "{} allocated {} times while routing ({} bytes); expected {expected:?}",
            scenario.diagnostic_name(),
            stats.routing.allocations,
            stats.routing.bytes,
        );
        assert_eq!(
            stats.observing.allocations,
            expected.observing(),
            "{} allocated {} times while observing its body ({} bytes); expected {expected:?}",
            scenario.diagnostic_name(),
            stats.observing.allocations,
            stats.observing.bytes,
        );
    }
}
