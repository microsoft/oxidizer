// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared concurrent, CPU-bound throughput fixtures.
//
// WHAT THIS MEASURES. Complete in-process request dispatch, path-capture
// extraction, an identical deterministic CPU-bound handler, response
// conversion, and complete response-body observation, run concurrently on
// several worker threads, reported as requests per second.
//
// WHAT THIS IS NOT. It is not a transport benchmark. No socket, connection
// handling, HTTP parsing, or serialization is involved, and no framework's
// own server is started. `docs/PERF.md` records why transport equality is not
// controllable across these five frameworks and what was measured instead.
//
// CONCURRENCY MODEL. Every target runs the same share-nothing thread-per-core
// structure: `workers` OS threads, each owning one current-thread Tokio
// runtime and its own application instance, each running `slots` concurrent
// local tasks, each task issuing `requests_per_slot` requests in sequence.
// A multi-threaded Tokio runtime is deliberately *not* used: Routerama's
// generated futures and Actix Web's handlers are both `!Send` by design, so a
// work-stealing runtime cannot host either without changing what is being
// measured. Because in-process dispatch has no I/O await point, the slots of
// one worker interleave at task boundaries rather than mid-request; they model
// connection multiplexing and scheduler cost, and the parallelism comes from
// the worker threads.
//
// FAIRNESS. Every target calls the identical `#[inline(never)]` `cpu_work`
// function with the identical rounds and the identical per-request seed, and
// every target observes the complete response body. Applications, runtimes,
// per-slot connections, and every request value are constructed outside the
// timed region; a barrier releases all workers together so no worker is timed
// while another is still preparing. The `handler_only` row is a control, not a
// framework: it runs the same CPU work and the same body digest with no
// routing at all, so the framework rows can be read against the application
// floor they share.
//
// SHAPE. The default is 4 workers x 8 slots x 128 requests = 4,096 requests
// per timed batch, overridable with `ROUTERAMA_THROUGHPUT_WORKERS`,
// `ROUTERAMA_THROUGHPUT_SLOTS`, and `ROUTERAMA_THROUGHPUT_REQUESTS` so a host
// with fewer usable cores can report the shape it actually ran. Absolute rates
// move with the shape; `docs/PERF.md` publishes a sweep of it.

use std::hint::black_box;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Barrier, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use tokio::runtime::{Builder, Runtime};
use tokio::task::LocalSet;
use tower_service::Service as TowerService;
use warp::{Filter as _, Reply as _};

/// The application work every target performs per request.
///
/// This is one `#[inline(never)]` function, called with the same rounds and
/// the same seed by all six targets, so no target can specialize, constant
/// fold, or skip it.
#[inline(never)]
fn cpu_work(seed: u64, rounds: u32) -> u64 {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    for round in 0..rounds {
        state ^= u64::from(round).rotate_left(17);
        state = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        state ^= state >> 29;
    }
    black_box(state)
}

/// How much CPU work each request performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Workload {
    /// Work comparable with the dispatch cost itself, so framework overhead is
    /// a large share of each request.
    Light,
    /// An order of magnitude more work, where the plan expects results to
    /// converge because the application dominates.
    Heavy,
}

impl Workload {
    const ALL: [Self; 2] = [Self::Light, Self::Heavy];

    const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Heavy => "heavy",
        }
    }

    /// Rounds were calibrated on the recording host so that the light
    /// workload's application cost is close to a framework's own dispatch
    /// cost, and the heavy workload's is roughly ten times that.
    const fn rounds(self) -> u32 {
        match self {
            Self::Light => 160,
            Self::Heavy => 1600,
        }
    }

    const fn path_segment(self) -> &'static str {
        self.name()
    }
}

/// Produces the response body every target returns for one request.
fn work_body(workload: Workload, seed: u64) -> String {
    format!("{:016x}", cpu_work(seed, workload.rounds()))
}

/// The digest each observed response contributes to a batch checksum.
///
/// Addition of per-request digests is order independent, so a checksum does
/// not depend on how the runtime happened to interleave slots.
fn digest(status: u16, body: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in body {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= u64::from(status);
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

/// The seed request number `index` of slot `slot` carries.
fn seed_for(slot: usize, index: usize, requests_per_slot: usize) -> u64 {
    u64::try_from(slot * requests_per_slot + index).expect("throughput seeds fit in u64")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The no-framework control: the same CPU work and the same digest with no
    /// routing, extraction, or response conversion.
    HandlerOnly,
    Routerama,
    Axum,
    ActixWeb,
    Rocket,
    Warp,
}

impl Target {
    const ALL: [Self; 6] = [
        Self::HandlerOnly,
        Self::Routerama,
        Self::Axum,
        Self::ActixWeb,
        Self::Rocket,
        Self::Warp,
    ];

    /// The five frameworks, excluding the application-floor control.
    const FRAMEWORKS: [Self; 5] = [Self::Routerama, Self::Axum, Self::ActixWeb, Self::Rocket, Self::Warp];

    const fn name(self) -> &'static str {
        match self {
            Self::HandlerOnly => "handler_only",
            Self::Routerama => "routerama",
            Self::Axum => "axum",
            Self::ActixWeb => "actix_web",
            Self::Rocket => "rocket",
            Self::Warp => "warp",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::HandlerOnly => 0,
            Self::Routerama => 1,
            Self::Axum => 2,
            Self::ActixWeb => 3,
            Self::Rocket => 4,
            Self::Warp => 5,
        }
    }
}

/// The shape of one timed batch.
#[derive(Clone, Copy, Debug)]
struct BatchPlan {
    target: Target,
    workload: Workload,
    slots: usize,
    requests_per_slot: usize,
}

impl BatchPlan {
    const fn requests_per_worker(&self) -> usize {
        self.slots * self.requests_per_slot
    }
}

/// What one worker thread observed for one batch.
#[derive(Clone, Copy, Debug)]
struct BatchOutcome {
    elapsed: Duration,
    checksum: u64,
}

/// The runtime shape of the whole measurement, overridable so a host with
/// fewer usable cores can report what it actually ran.
#[derive(Clone, Copy, Debug)]
struct Shape {
    workers: usize,
    slots: usize,
    requests_per_slot: usize,
}

impl Shape {
    fn from_environment() -> Self {
        Self {
            workers: environment_usize("ROUTERAMA_THROUGHPUT_WORKERS", 4),
            slots: environment_usize("ROUTERAMA_THROUGHPUT_SLOTS", 8),
            requests_per_slot: environment_usize("ROUTERAMA_THROUGHPUT_REQUESTS", 128),
        }
    }

    const fn requests_per_batch(&self) -> usize {
        self.workers * self.slots * self.requests_per_slot
    }

    fn plan(&self, target: Target, workload: Workload) -> BatchPlan {
        BatchPlan {
            target,
            workload,
            slots: self.slots,
            requests_per_slot: self.requests_per_slot,
        }
    }
}

fn environment_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn shape() -> Shape {
    static SHAPE: OnceLock<Shape> = OnceLock::new();
    *SHAPE.get_or_init(Shape::from_environment)
}

fn process_lifetime<T: 'static>(value: T) -> &'static T {
    // Keep every target's runtime and routing state alive symmetrically so a
    // timed batch can never perform final-reference teardown in region.
    Box::leak(Box::new(value))
}

/// Runs one batch's slots as concurrent local tasks on one current-thread
/// runtime and returns the batch checksum.
fn drive_slots<C, R, F, Fut>(runtime: &Runtime, prepared: Vec<(C, Vec<R>)>, slot: F) -> u64
where
    C: 'static,
    R: 'static,
    F: Fn(C, Vec<R>) -> Fut,
    Fut: Future<Output = u64> + 'static,
{
    let local = LocalSet::new();
    runtime.block_on(local.run_until(async move {
        let mut handles = Vec::with_capacity(prepared.len());
        for (connection, requests) in prepared {
            handles.push(tokio::task::spawn_local(slot(connection, requests)));
        }
        let mut checksum = 0_u64;
        for handle in handles {
            checksum = checksum.wrapping_add(handle.await.expect("a throughput slot task never panics"));
        }
        checksum
    }))
}

/// Builds one slot's requests. Always called outside the timed region.
fn slot_requests<R>(plan: BatchPlan, slot: usize, build: impl Fn(u64) -> R) -> Vec<R> {
    (0..plan.requests_per_slot)
        .map(|index| build(seed_for(slot, index, plan.requests_per_slot)))
        .collect()
}

type BatchRunner = Box<dyn Fn(BatchPlan, &Barrier) -> BatchOutcome>;

/// Times `run` after every worker has finished preparing.
fn timed(barrier: &Barrier, run: impl FnOnce() -> u64) -> BatchOutcome {
    barrier.wait();
    let start = Instant::now();
    let checksum = run();
    BatchOutcome {
        elapsed: start.elapsed(),
        checksum,
    }
}

fn work_path(workload: Workload, seed: u64) -> String {
    format!("/work/{}/{seed}", workload.path_segment())
}

// The no-framework control.

fn build_handler_only_runner(runtime: &'static Runtime) -> BatchRunner {
    Box::new(move |plan, barrier| {
        let workload = plan.workload;
        let prepared: Vec<((), Vec<u64>)> = (0..plan.slots)
            .map(|slot| ((), slot_requests(plan, slot, |seed| seed)))
            .collect();
        timed(barrier, || {
            drive_slots(runtime, prepared, move |(), seeds| async move {
                let mut checksum = 0_u64;
                for seed in seeds {
                    let body = work_body(workload, seed);
                    checksum = checksum.wrapping_add(digest(200, body.as_bytes()));
                }
                checksum
            })
        })
    })
}

// Routerama.

struct RouteramaThroughput;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl RouteramaThroughput {
    #[route(GET, "/work/light/{seed}")]
    async fn light(&self, seed: u64) -> String {
        work_body(Workload::Light, seed)
    }

    #[route(GET, "/work/heavy/{seed}")]
    async fn heavy(&self, seed: u64) -> String {
        work_body(Workload::Heavy, seed)
    }

    #[route(GET, "/fixture/01")]
    async fn filler_01(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/02")]
    async fn filler_02(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/03")]
    async fn filler_03(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/04")]
    async fn filler_04(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/05")]
    async fn filler_05(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/06")]
    async fn filler_06(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/07")]
    async fn filler_07(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/08")]
    async fn filler_08(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/09")]
    async fn filler_09(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/10")]
    async fn filler_10(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/11")]
    async fn filler_11(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/12")]
    async fn filler_12(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/13")]
    async fn filler_13(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/fixture/14")]
    async fn filler_14(&self) -> &'static str {
        "filler"
    }
}

fn build_routerama_runner(runtime: &'static Runtime) -> BatchRunner {
    Box::new(move |plan, barrier| {
        let prepared: Vec<(RouteramaThroughput, Vec<http::Request<()>>)> = (0..plan.slots)
            .map(|slot| {
                (
                    RouteramaThroughput,
                    slot_requests(plan, slot, |seed| {
                        http::Request::get(work_path(plan.workload, seed))
                            .body(())
                            .expect("the throughput request metadata is valid")
                    }),
                )
            })
            .collect();
        timed(barrier, || {
            drive_slots(runtime, prepared, |application, requests| async move {
                let mut checksum = 0_u64;
                for request in requests {
                    let response = application.route(request, &()).await;
                    let status = response.status().as_u16();
                    let body = response
                        .into_body()
                        .collect()
                        .await
                        .expect("the generated response body is infallible")
                        .to_bytes();
                    checksum = checksum.wrapping_add(digest(status, &body));
                }
                checksum
            })
        })
    })
}

// Axum.

fn axum_work(seed: u64, workload: Workload) -> String {
    work_body(workload, seed)
}

async fn axum_light(axum::extract::Path(seed): axum::extract::Path<u64>) -> String {
    axum_work(seed, Workload::Light)
}

async fn axum_heavy(axum::extract::Path(seed): axum::extract::Path<u64>) -> String {
    axum_work(seed, Workload::Heavy)
}

async fn axum_filler() -> &'static str {
    "filler"
}

fn build_axum_router() -> axum::Router {
    use axum::routing::get;

    let mut router = axum::Router::new()
        .route("/work/light/{seed}", get(axum_light))
        .route("/work/heavy/{seed}", get(axum_heavy));
    for filler in FILLER_PATHS {
        router = router.route(filler, get(axum_filler));
    }
    router.with_state(())
}

fn build_axum_runner(runtime: &'static Runtime) -> BatchRunner {
    let router = process_lifetime(build_axum_router());
    Box::new(move |plan, barrier| {
        let prepared: Vec<(axum::Router, Vec<http::Request<axum::body::Body>>)> = (0..plan.slots)
            .map(|slot| {
                (
                    router.clone(),
                    slot_requests(plan, slot, |seed| {
                        http::Request::get(work_path(plan.workload, seed))
                            .body(axum::body::Body::empty())
                            .expect("the throughput request metadata is valid")
                    }),
                )
            })
            .collect();
        timed(barrier, || {
            drive_slots(runtime, prepared, |mut connection, requests| async move {
                let mut checksum = 0_u64;
                for request in requests {
                    let response = TowerService::call(&mut connection, request)
                        .await
                        .expect("the Axum router is infallible");
                    let status = response.status().as_u16();
                    let body = response
                        .into_body()
                        .collect()
                        .await
                        .expect("the Axum response body is in memory")
                        .to_bytes();
                    checksum = checksum.wrapping_add(digest(status, &body));
                }
                checksum
            })
        })
    })
}

// Actix Web.

async fn actix_light(path: actix_web::web::Path<u64>) -> String {
    work_body(Workload::Light, path.into_inner())
}

async fn actix_heavy(path: actix_web::web::Path<u64>) -> String {
    work_body(Workload::Heavy, path.into_inner())
}

async fn actix_filler() -> &'static str {
    "filler"
}

fn build_actix_web_runner(runtime: &'static Runtime) -> BatchRunner {
    use actix_web::{App, test, web};

    let service = runtime.block_on(test::init_service({
        let mut app = App::new()
            .route("/work/light/{seed}", web::get().to(actix_light))
            .route("/work/heavy/{seed}", web::get().to(actix_heavy));
        for filler in FILLER_PATHS {
            app = app.route(filler, web::get().to(actix_filler));
        }
        app
    }));
    let service = process_lifetime(service);

    Box::new(move |plan, barrier| {
        let prepared: Vec<_> = (0..plan.slots)
            .map(|slot| {
                (
                    service,
                    slot_requests(plan, slot, |seed| {
                        test::TestRequest::get().uri(&work_path(plan.workload, seed)).to_request()
                    }),
                )
            })
            .collect();
        timed(barrier, || {
            drive_slots(runtime, prepared, |connection, requests| async move {
                let mut checksum = 0_u64;
                for request in requests {
                    let response = test::call_service(connection, request).await;
                    let status = response.status().as_u16();
                    let body = actix_web::body::to_bytes(response.into_body())
                        .await
                        .expect("the Actix Web response body is in memory");
                    checksum = checksum.wrapping_add(digest(status, &body));
                }
                checksum
            })
        })
    })
}

// Rocket.

#[rocket::get("/work/light/<seed>")]
fn rocket_light(seed: &str) -> String {
    let seed = seed.parse::<u64>().expect("the throughput fixture supplies numeric seeds");
    work_body(Workload::Light, seed)
}

#[rocket::get("/work/heavy/<seed>")]
fn rocket_heavy(seed: &str) -> String {
    let seed = seed.parse::<u64>().expect("the throughput fixture supplies numeric seeds");
    work_body(Workload::Heavy, seed)
}

macro_rules! rocket_filler {
    ($name:ident, $path:literal) => {
        #[rocket::get($path)]
        fn $name() -> &'static str {
            "filler"
        }
    };
}

rocket_filler!(rocket_filler_01, "/fixture/01");
rocket_filler!(rocket_filler_02, "/fixture/02");
rocket_filler!(rocket_filler_03, "/fixture/03");
rocket_filler!(rocket_filler_04, "/fixture/04");
rocket_filler!(rocket_filler_05, "/fixture/05");
rocket_filler!(rocket_filler_06, "/fixture/06");
rocket_filler!(rocket_filler_07, "/fixture/07");
rocket_filler!(rocket_filler_08, "/fixture/08");
rocket_filler!(rocket_filler_09, "/fixture/09");
rocket_filler!(rocket_filler_10, "/fixture/10");
rocket_filler!(rocket_filler_11, "/fixture/11");
rocket_filler!(rocket_filler_12, "/fixture/12");
rocket_filler!(rocket_filler_13, "/fixture/13");
rocket_filler!(rocket_filler_14, "/fixture/14");

#[expect(
    clippy::redundant_type_annotations,
    reason = "Rocket's routes macro emits explicit internal types"
)]
fn build_rocket_runner(runtime: &'static Runtime) -> BatchRunner {
    use rocket::local::asynchronous::Client;

    let rocket = rocket::custom(rocket::Config::figment().merge(("log_level", rocket::config::LogLevel::Off))).mount(
        "/",
        rocket::routes![
            rocket_light,
            rocket_heavy,
            rocket_filler_01,
            rocket_filler_02,
            rocket_filler_03,
            rocket_filler_04,
            rocket_filler_05,
            rocket_filler_06,
            rocket_filler_07,
            rocket_filler_08,
            rocket_filler_09,
            rocket_filler_10,
            rocket_filler_11,
            rocket_filler_12,
            rocket_filler_13,
            rocket_filler_14,
        ],
    );
    let client = process_lifetime(
        runtime
            .block_on(Client::untracked(rocket))
            .expect("the Rocket throughput application ignites"),
    );

    Box::new(move |plan, barrier| {
        let prepared: Vec<_> = (0..plan.slots)
            .map(|slot| {
                (
                    (),
                    slot_requests(plan, slot, |seed| client.get(work_path(plan.workload, seed))),
                )
            })
            .collect();
        timed(barrier, || {
            drive_slots(runtime, prepared, |(), requests| async move {
                let mut checksum = 0_u64;
                for request in requests {
                    let response = request.dispatch().await;
                    let status = response.status().code;
                    let body = response.into_bytes().await.unwrap_or_default();
                    checksum = checksum.wrapping_add(digest(status, &body));
                }
                checksum
            })
        })
    })
}

// Warp.

type WarpRoutes = warp::filters::BoxedFilter<(warp::reply::Response,)>;

fn warp_work(workload: Workload) -> WarpRoutes {
    warp::get()
        .and(warp::path("work"))
        .and(warp::path(workload.path_segment()))
        .and(warp::path::param::<u64>())
        .and(warp::path::end())
        .map(move |seed: u64| {
            warp::reply::with_status(work_body(workload, seed), warp::http::StatusCode::OK).into_response()
        })
        .boxed()
}

fn warp_filler(segment: &'static str) -> WarpRoutes {
    warp::get()
        .and(warp::path("fixture"))
        .and(warp::path(segment))
        .and(warp::path::end())
        .map(|| warp::reply::with_status("filler", warp::http::StatusCode::OK).into_response())
        .boxed()
}

fn build_warp_routes() -> WarpRoutes {
    let mut routes = warp_work(Workload::Light);
    routes = routes.or(warp_work(Workload::Heavy)).unify().boxed();
    for filler in FILLER_SEGMENTS {
        routes = routes.or(warp_filler(filler)).unify().boxed();
    }
    routes
        .or(warp::any()
            .map(|| warp::reply::with_status("", warp::http::StatusCode::NOT_FOUND).into_response())
            .boxed())
        .unify()
        .boxed()
}

fn build_warp_runner(runtime: &'static Runtime) -> BatchRunner {
    let service = process_lifetime(warp::service(build_warp_routes()));
    Box::new(move |plan, barrier| {
        let prepared: Vec<_> = (0..plan.slots)
            .map(|slot| {
                (
                    service.clone(),
                    slot_requests(plan, slot, |seed| {
                        http::Request::get(work_path(plan.workload, seed))
                            .body(Empty::<Bytes>::new())
                            .expect("the throughput request metadata is valid")
                    }),
                )
            })
            .collect();
        timed(barrier, || {
            drive_slots(runtime, prepared, |mut connection, requests| async move {
                let mut checksum = 0_u64;
                for request in requests {
                    let response = TowerService::call(&mut connection, request)
                        .await
                        .unwrap_or_else(|error: std::convert::Infallible| match error {});
                    let status = response.status().as_u16();
                    let body = response
                        .into_body()
                        .collect()
                        .await
                        .expect("the Warp response body is in memory")
                        .to_bytes();
                    checksum = checksum.wrapping_add(digest(status, &body));
                }
                checksum
            })
        })
    })
}

const FILLER_SEGMENTS: [&str; 14] = [
    "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14",
];

const FILLER_PATHS: [&str; 14] = [
    "/fixture/01",
    "/fixture/02",
    "/fixture/03",
    "/fixture/04",
    "/fixture/05",
    "/fixture/06",
    "/fixture/07",
    "/fixture/08",
    "/fixture/09",
    "/fixture/10",
    "/fixture/11",
    "/fixture/12",
    "/fixture/13",
    "/fixture/14",
];

/// One worker thread's six applications, all built on its own runtime.
fn build_runners() -> [BatchRunner; 6] {
    let runtime = process_lifetime(
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("the throughput Tokio runtime builds"),
    );
    [
        build_handler_only_runner(runtime),
        build_routerama_runner(runtime),
        build_axum_runner(runtime),
        build_actix_web_runner(runtime),
        build_rocket_runner(runtime),
        build_warp_runner(runtime),
    ]
}

enum Command {
    Run(BatchPlan),
    Stop,
}

/// The worker pool. Applications and runtimes are built once per worker
/// thread, so no batch pays construction or teardown.
struct Pool {
    shape: Shape,
    commands: Vec<Sender<Command>>,
    /// Only the measuring thread ever collects outcomes; the lock exists so
    /// the pool can live in a process-lifetime static.
    outcomes: Mutex<Receiver<BatchOutcome>>,
    barrier: &'static Barrier,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Pool {
    fn new(shape: Shape) -> Self {
        let barrier: &'static Barrier = process_lifetime(Barrier::new(shape.workers));
        let (outcome_sender, outcomes) = channel();
        let mut commands = Vec::with_capacity(shape.workers);
        let mut threads = Vec::with_capacity(shape.workers);
        for worker in 0..shape.workers {
            let (command_sender, command_receiver) = channel();
            let outcome_sender = outcome_sender.clone();
            commands.push(command_sender);
            threads.push(
                std::thread::Builder::new()
                    .name(format!("throughput-{worker}"))
                    .spawn(move || {
                        let runners = build_runners();
                        while let Ok(Command::Run(plan)) = command_receiver.recv() {
                            let outcome = runners[plan.target.index()](plan, barrier);
                            if outcome_sender.send(outcome).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("a throughput worker thread starts"),
            );
        }
        let pool = Self {
            shape,
            commands,
            outcomes: Mutex::new(outcomes),
            barrier,
            threads,
        };
        pool.warm_up();
        pool
    }

    /// How many batches per target and workload are run, and discarded, before
    /// anything is reported.
    const WARM_UP_BATCHES: usize = 5;

    /// Runs every target and workload so lazy initialization, allocator arenas,
    /// branch predictors, and instruction caches are warm before any reported
    /// measurement.
    fn warm_up(&self) {
        for _ in 0..Self::WARM_UP_BATCHES {
            for target in Target::ALL {
                for workload in Workload::ALL {
                    let _ = self.run_batch(target, workload);
                }
            }
        }
    }

    /// Runs one batch on every worker and returns its wall-clock duration.
    ///
    /// The reported duration is the slowest worker's, which is the time in
    /// which `shape.requests_per_batch()` requests were completed.
    fn run_batch(&self, target: Target, workload: Workload) -> Batch {
        let plan = self.shape.plan(target, workload);
        for command in &self.commands {
            command.send(Command::Run(plan)).expect("a throughput worker accepts work");
        }
        let outcomes = self.outcomes.lock().expect("the throughput outcome channel is never poisoned");
        let mut elapsed = Duration::ZERO;
        let mut checksum = 0_u64;
        for _ in 0..self.shape.workers {
            let outcome = outcomes.recv().expect("a throughput worker reports its batch");
            elapsed = elapsed.max(outcome.elapsed);
            checksum = checksum.wrapping_add(outcome.checksum);
        }
        Batch {
            requests: self.shape.requests_per_batch(),
            elapsed,
            checksum,
        }
    }

}

/// Runs one batch on the calling thread only, for equivalence checks that must
/// not depend on how many workers are configured and must not start the pool.
fn run_single_worker_batch(target: Target, workload: Workload, requests_per_slot: usize) -> Batch {
    let plan = BatchPlan {
        target,
        workload,
        slots: 1,
        requests_per_slot,
    };
    let alone = Barrier::new(1);
    let outcome = SINGLE_WORKER_RUNNERS.with(|runners| runners[plan.target.index()](plan, &alone));
    Batch {
        requests: plan.requests_per_worker(),
        elapsed: outcome.elapsed,
        checksum: outcome.checksum,
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        for command in &self.commands {
            let _ = command.send(Command::Stop);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

thread_local! {
    /// Applications for equivalence checks that run on the calling thread.
    static SINGLE_WORKER_RUNNERS: [BatchRunner; 6] = build_runners();
}

#[derive(Clone, Copy, Debug)]
struct Batch {
    requests: usize,
    elapsed: Duration,
    checksum: u64,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "request counts are small enough that the f64 conversion is exact"
)]
impl Batch {
    fn requests_per_second(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.requests as f64 / self.elapsed.as_secs_f64()
    }

    fn nanoseconds_per_request(&self) -> f64 {
        if self.requests == 0 {
            return 0.0;
        }
        self.elapsed.as_secs_f64() * 1e9 / self.requests as f64
    }
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(shape()))
}

/// Asserts that every target computes the identical CPU result and returns the
/// identical responses for the identical seeds.
fn assert_equivalent() {
    const REQUESTS: usize = 8;
    for workload in Workload::ALL {
        let expected: u64 = (0..REQUESTS)
            .map(|index| {
                let seed = seed_for(0, index, REQUESTS);
                digest(200, work_body(workload, seed).as_bytes())
            })
            .fold(0_u64, u64::wrapping_add);
        for target in Target::ALL {
            let batch = run_single_worker_batch(target, workload, REQUESTS);
            assert_eq!(
                batch.checksum,
                expected,
                "{} produced a different {} response set; every target must return the identical CPU result",
                target.name(),
                workload.name()
            );
        }
    }
}

/// Asserts that the CPU work itself is what the fixture claims: deterministic,
/// seed dependent, and heavier for the heavy workload.
fn assert_cpu_work_is_deterministic_and_scaled() {
    for workload in Workload::ALL {
        assert_eq!(
            cpu_work(7, workload.rounds()),
            cpu_work(7, workload.rounds()),
            "the {} workload must be deterministic",
            workload.name()
        );
        assert_ne!(
            cpu_work(7, workload.rounds()),
            cpu_work(8, workload.rounds()),
            "the {} workload must depend on its seed",
            workload.name()
        );
    }
    assert!(
        Workload::Heavy.rounds() > Workload::Light.rounds() * 4,
        "the heavy workload must be an order of magnitude heavier than the light one"
    );
    assert_ne!(
        cpu_work(7, Workload::Light.rounds()),
        cpu_work(7, Workload::Heavy.rounds()),
        "the two workloads must not collapse into the same computation"
    );
}

/// One target's repeated measurements for one workload.
#[derive(Clone, Debug)]
struct Measurement {
    target: Target,
    workload: Workload,
    requests_per_second: Vec<f64>,
    nanoseconds_per_request: Vec<f64>,
}

impl Measurement {
    fn median(values: &[f64]) -> f64 {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted.get(sorted.len() / 2).copied().unwrap_or_default()
    }

    fn minimum(values: &[f64]) -> f64 {
        values.iter().copied().fold(f64::INFINITY, f64::min)
    }

    fn maximum(values: &[f64]) -> f64 {
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }

    fn median_requests_per_second(&self) -> f64 {
        Self::median(&self.requests_per_second)
    }
}

/// Runs `repeats` batches per target and workload and returns the results.
fn measure(repeats: usize) -> Vec<Measurement> {
    let mut measurements = Vec::with_capacity(Target::ALL.len() * Workload::ALL.len());
    for workload in Workload::ALL {
        for target in Target::ALL {
            let mut requests_per_second = Vec::with_capacity(repeats);
            let mut nanoseconds_per_request = Vec::with_capacity(repeats);
            for _ in 0..repeats {
                let batch = pool().run_batch(target, workload);
                requests_per_second.push(batch.requests_per_second());
                nanoseconds_per_request.push(batch.nanoseconds_per_request());
            }
            measurements.push(Measurement {
                target,
                workload,
                requests_per_second,
                nanoseconds_per_request,
            });
        }
    }
    measurements
}
