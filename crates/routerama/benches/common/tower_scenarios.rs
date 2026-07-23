// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Direct routing compared with Tower's exact and send-boxed body boundaries.
// Setup and closure boxing occur in `prepare`; measured work includes the
// common indirect call and any selected body erasure.

use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::{future::Future, mem::size_of_val};

use bytes::Bytes;
use http::StatusCode;
use http_body::Body as HttpBody;
use routerama::response::{Body, Response};
use routerama::route::tower::RouteService;
use routerama::route::{Request, router};
use tower_service::Service as TowerService;

const SERVED: &[u8] = b"served";

#[derive(Clone)]
struct AppState {
    shared: Arc<SharedState>,
}

struct SharedState {
    deployment: &'static str,
    routing_seed: [u64; 16],
}

/// A zero-sized generated router: cloning it into each adapter call is free.
#[derive(Clone, Copy)]
struct Api;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, tower)]
impl Api {
    #[route(GET, "/health")]
    async fn health(&self, state: routerama::route::State<AppState>) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(state.shared.deployment);
        let _ = std::hint::black_box(state.shared.routing_seed[0]);
        (StatusCode::OK, Bytes::from_static(SERVED))
    }
}

fn state() -> Arc<AppState> {
    Arc::new(AppState {
        shared: Arc::new(SharedState {
            deployment: "west",
            routing_seed: [0xfeed_face_dead_beef; 16],
        }),
    })
}

fn request() -> Request<Body> {
    Request::get("/health")
        .body(Body::empty())
        .expect("the Tower benchmark request metadata is valid")
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

// Expand the identical poll sequence at each call site. A generic helper was
// selectively outlined for only the opaque generated future, which measured
// code placement instead of the response boundary.
macro_rules! run_ready {
    ($future:expr) => {{
        // Stack-pin to avoid allocator noise on the measured route path.
        let mut future = pin!($future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("the in-memory generated route future must complete in one poll"),
        }
    }};
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
            Poll::Ready(Some(Err(_))) => panic!("the Tower evidence bodies never fail"),
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
    DirectRoute,
    RouteServiceExactBody,
    GeneratedExactTower,
    RouteServiceSendBoxBody,
}

impl Scenario {
    const ALL: [Self; 4] = [
        Self::DirectRoute,
        Self::RouteServiceExactBody,
        Self::GeneratedExactTower,
        Self::RouteServiceSendBoxBody,
    ];

    const fn group(self) -> &'static str {
        match self {
            Self::DirectRoute | Self::RouteServiceExactBody | Self::GeneratedExactTower | Self::RouteServiceSendBoxBody => "dispatch",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::DirectRoute => "direct_route",
            Self::RouteServiceExactBody => "route_service_exact_body",
            Self::GeneratedExactTower => "generated_exact_tower",
            Self::RouteServiceSendBoxBody => "route_service_send_box_body",
        }
    }

    fn diagnostic_name(self) -> String {
        format!("{}/{}", self.group(), self.name())
    }
}

type PreparedScenario = Box<dyn FnOnce() -> Observation>;

fn prepare(scenario: Scenario) -> PreparedScenario {
    let state = state();
    let request = request();
    match scenario {
        Scenario::DirectRoute => Box::new(move || observe(run_ready!(Api.route(request, state.as_ref())))),
        Scenario::RouteServiceExactBody => {
            let mut service = RouteService::new(
                Api,
                state,
                |api: Api, state: Arc<AppState>, request: Request<Body>| async move {
                    api.route(request, state.as_ref()).await
                },
            );
            Box::new(move || {
                observe(
                    run_ready!(TowerService::call(&mut service, request))
                        .expect("generated routing through the Tower adapter is infallible"),
                )
            })
        }
        Scenario::GeneratedExactTower => {
            let mut service = Api::tower_service::<Body, _, _>(Api, state);
            Box::new(move || {
                observe(
                    run_ready!(TowerService::call(&mut service, request))
                        .expect("generated exact Tower routing is infallible"),
                )
            })
        }
        Scenario::RouteServiceSendBoxBody => {
            let mut service = RouteService::new(
                Api,
                state,
                |api: Api, state: Arc<AppState>, request: Request<Body>| async move {
                    api.route(request, state.as_ref()).await
                },
            )
            .send_boxed_body();
            Box::new(move || {
                observe(
                    run_ready!(TowerService::call(&mut service, request))
                        .expect("generated routing through the Tower adapter is infallible"),
                )
            })
        }
    }
}

fn run_prepared(prepared: PreparedScenario) -> Observation {
    std::hint::black_box(prepared)()
}

fn expected() -> Observation {
    Observation {
        status: 200,
        body: Fingerprint::of(SERVED),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SizeDiagnostics {
    exact_future: usize,
    exact_response: usize,
    exact_body: usize,
    generated_future: usize,
    generated_response: usize,
    generated_body: usize,
    send_boxed_future: usize,
    send_boxed_response: usize,
    send_boxed_body: usize,
}

fn value_sizes<F, B>(future: F) -> (usize, usize, usize)
where
    F: Future<Output = Result<Response<B>, core::convert::Infallible>>,
{
    let future_size = size_of_val(&future);
    let response = run_ready!(future).expect("Tower routing is infallible");
    let response_size = size_of_val(&response);
    let body = response.into_body();
    let body_size = size_of_val(&body);
    std::hint::black_box(body);
    (future_size, response_size, body_size)
}

fn size_diagnostics() -> SizeDiagnostics {
    let state = state();
    let mut exact = RouteService::new(
        Api,
        Arc::clone(&state),
        |api: Api, state: Arc<AppState>, request: Request<Body>| async move {
            api.route(request, state.as_ref()).await
        },
    );
    let exact = value_sizes(TowerService::call(&mut exact, request()));

    let mut generated = Api::tower_service::<Body, _, _>(Api, Arc::clone(&state));
    let generated = value_sizes(TowerService::call(&mut generated, request()));

    let mut send_boxed = RouteService::new(
        Api,
        state,
        |api: Api, state: Arc<AppState>, request: Request<Body>| async move {
            api.route(request, state.as_ref()).await
        },
    )
    .send_boxed_body();
    let send_boxed = value_sizes(TowerService::call(&mut send_boxed, request()));

    SizeDiagnostics {
        exact_future: exact.0,
        exact_response: exact.1,
        exact_body: exact.2,
        generated_future: generated.0,
        generated_response: generated.1,
        generated_body: generated.2,
        send_boxed_future: send_boxed.0,
        send_boxed_response: send_boxed.1,
        send_boxed_body: send_boxed.2,
    }
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        assert_eq!(
            run_prepared(prepare(scenario)),
            expected(),
            "{} changed its routed response; every Tower scenario must stay response-identical",
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

fn allocation_diagnostics() -> [(Scenario, AllocationStats); 4] {
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
            let _span = operation.measure_thread().iterations(1);
            std::hint::black_box(run_prepared(prepared));
        }
        let report = session.to_report();
        (scenario, report_stats(&report, "measured"))
    })
}
