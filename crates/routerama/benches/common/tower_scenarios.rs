// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared Tower-adapter scenarios. The same generated static request is driven
// three ways: directly through the generated `route` entry, through
// `RouteService`'s default `ExactBody` boundary, and through the explicit
// `SendBoxBody` boundary that a `Send` transport requires.
//
// `RouteService`'s `Call` type parameter is an unnameable closure, so each
// scenario is prepared as a boxed `FnOnce` exactly like the five-framework
// fixtures. Service construction, state, request creation, and the boxing all
// happen in `prepare`; every row pays the same one indirect call inside the
// measured region.
//
// The `SendBoxBody` row deliberately includes its one body allocation, because
// that allocation is exactly what opting into erasure buys.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::StatusCode;
use http_body::Body as HttpBody;
use routerama::response::{Body, Response};
use routerama::route::tower::RouteService;
use routerama::route::{Request, router};
use tower_service::Service as TowerService;

const SERVED: &[u8] = b"served";

#[derive(Clone, Copy)]
struct AppState {
    deployment: &'static str,
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
#[router(state = AppState)]
impl Api {
    #[route(GET, "/health")]
    async fn health(&self, state: routerama::route::State<AppState>) -> (StatusCode, Bytes) {
        let _ = std::hint::black_box(state.deployment);
        (StatusCode::OK, Bytes::from_static(SERVED))
    }
}

fn state() -> AppState {
    AppState { deployment: "west" }
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
    RouteServiceSendBoxBody,
}

impl Scenario {
    const ALL: [Self; 3] = [Self::DirectRoute, Self::RouteServiceExactBody, Self::RouteServiceSendBoxBody];

    const fn group(self) -> &'static str {
        match self {
            Self::DirectRoute | Self::RouteServiceExactBody | Self::RouteServiceSendBoxBody => "dispatch",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::DirectRoute => "direct_route",
            Self::RouteServiceExactBody => "route_service_exact_body",
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
        Scenario::DirectRoute => Box::new(move || observe(run_ready(Api.route(request, &state)))),
        Scenario::RouteServiceExactBody => {
            let mut service = RouteService::new(Api, state, |api: Api, state: AppState, request: Request<Body>| async move {
                api.route(request, &state).await
            });
            Box::new(move || {
                observe(
                    run_ready(TowerService::call(&mut service, request))
                        .expect("generated routing through the Tower adapter is infallible"),
                )
            })
        }
        Scenario::RouteServiceSendBoxBody => {
            let mut service = RouteService::new(Api, state, |api: Api, state: AppState, request: Request<Body>| async move {
                api.route(request, &state).await
            })
            .send_boxed_body();
            Box::new(move || {
                observe(
                    run_ready(TowerService::call(&mut service, request))
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

fn allocation_diagnostics() -> [(Scenario, AllocationStats); 3] {
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
