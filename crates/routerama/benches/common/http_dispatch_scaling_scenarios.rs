// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared, network-free route-set scaling fixtures. The generated source gives
// every framework the same literal paths, registration order, and responses at
// 16, 128, and 1,024 routes. Construction, request creation, equivalence
// checking, warmup, and process-lifetime teardown are outside measured calls.

use std::cell::RefCell;
use std::convert::Infallible;
use std::future::Future;
use std::io::Cursor;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use tokio::runtime::{Builder, Runtime};
use tower_service::Service as TowerService;
use warp::{Filter as _, Reply as _};

const MARKER_HEADER: &str = "x-route-id";

#[derive(Clone, Copy, Debug)]
struct RouteSpec {
    path: &'static str,
    set_segment: &'static str,
    route_segment: &'static str,
    response: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteSetSize {
    Routes16,
    Routes128,
    Routes1024,
}

impl RouteSetSize {
    const ALL: [Self; 3] = [Self::Routes16, Self::Routes128, Self::Routes1024];

    const fn name(self) -> &'static str {
        match self {
            Self::Routes16 => "routes_16",
            Self::Routes128 => "routes_128",
            Self::Routes1024 => "routes_1024",
        }
    }

    const fn routes(self) -> &'static [RouteSpec] {
        match self {
            Self::Routes16 => &ROUTES_16,
            Self::Routes128 => &ROUTES_128,
            Self::Routes1024 => &ROUTES_1024,
        }
    }

    const fn miss_path(self) -> &'static str {
        match self {
            Self::Routes16 => MISS_16,
            Self::Routes128 => MISS_128,
            Self::Routes1024 => MISS_1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framework {
    Routerama,
    Axum,
    ActixWeb,
    Rocket,
    Warp,
}

impl Framework {
    const ALL: [Self; 5] = [
        Self::Routerama,
        Self::Axum,
        Self::ActixWeb,
        Self::Rocket,
        Self::Warp,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Routerama => "routerama",
            Self::Axum => "axum",
            Self::ActixWeb => "actix_web",
            Self::Rocket => "rocket",
            Self::Warp => "warp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    First,
    Middle,
    Last,
    Miss,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::First, Self::Middle, Self::Last, Self::Miss];

    const fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Middle => "middle",
            Self::Last => "last",
            Self::Miss => "miss",
        }
    }

    fn route(self, size: RouteSetSize) -> Option<RouteSpec> {
        let routes = size.routes();
        match self {
            Self::First => routes.first().copied(),
            Self::Middle => routes.get(routes.len() / 2).copied(),
            Self::Last => routes.last().copied(),
            Self::Miss => None,
        }
    }

    fn path(self, size: RouteSetSize) -> &'static str {
        self.route(size).map_or_else(|| size.miss_path(), |route| route.path)
    }

    fn expected(self, size: RouteSetSize) -> Observation {
        self.route(size).map_or_else(
            || Observation::new(404, None, b""),
            |route| {
                Observation::new(
                    200,
                    Some(route.response.as_bytes()),
                    route.response.as_bytes(),
                )
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    length: usize,
    hash: u64,
}

impl Fingerprint {
    fn of(bytes: &[u8]) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self {
            length: bytes.len(),
            hash,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    status: u16,
    marker: Option<Fingerprint>,
    body: Fingerprint,
}

impl Observation {
    fn new(status: u16, marker: Option<&[u8]>, body: &[u8]) -> Self {
        Self {
            status,
            marker: marker.map(Fingerprint::of),
            body: Fingerprint::of(body),
        }
    }
}

type PreparedCall = Box<dyn FnOnce() -> Observation>;
type CallFactory = Box<dyn Fn(Scenario) -> PreparedCall>;

fn process_lifetime<T: 'static>(value: T) -> &'static T {
    // Keep every framework's runtime and routing state alive symmetrically so
    // prepared calls can never perform final-reference teardown in-region.
    Box::leak(Box::new(value))
}

fn new_runtime() -> &'static Runtime {
    process_lifetime(
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("the benchmark Tokio runtime builds"),
    )
}

fn run_on_runtime<F>(runtime: &Runtime, future: F) -> F::Output
where
    F: Future,
{
    // Stack-pin to avoid allocator noise on the measured path.
    let future = std::pin::pin!(future);
    runtime.block_on(future)
}

async fn response_observation<B>(status: u16, marker: Option<Fingerprint>, body: B) -> Observation
where
    B: http_body::Body,
    B::Data: bytes::Buf,
    B::Error: std::fmt::Debug,
{
    let body = body
        .collect()
        .await
        .expect("the in-memory benchmark response body is infallible")
        .to_bytes();
    Observation {
        status,
        marker,
        body: Fingerprint::of(&body),
    }
}

// Routerama uses one generated static #[router] service per route-set size.

type RouteramaScalingResponse = (
    routerama::route::StatusCode,
    [(http::HeaderName, http::HeaderValue); 1],
    &'static str,
);

fn routerama_scaling_response(response: &'static str) -> RouteramaScalingResponse {
    (
        routerama::route::StatusCode::OK,
        [(
            http::HeaderName::from_static(MARKER_HEADER),
            http::HeaderValue::from_static(response),
        )],
        response,
    )
}

macro_rules! define_routerama_factory {
    ($factory:ident, $service:ident, $size:ident) => {
        fn $factory() -> CallFactory {
            let runtime = new_runtime();
            let service = process_lifetime($service);
            Box::new(move |scenario| {
                let request = http::Request::builder()
                    .method("GET")
                    .uri(scenario.path(RouteSetSize::$size))
                    .body(())
                    .expect("the generated benchmark request metadata is valid");
                Box::new(move || {
                    run_on_runtime(runtime, async move {
                        let response = service.route(request, &()).await;
                        let status = response.status().as_u16();
                        let marker = response
                            .headers()
                            .get(MARKER_HEADER)
                            .map(|value| Fingerprint::of(value.as_bytes()));
                        response_observation(status, marker, response.into_body()).await
                    })
                })
            })
        }
    };
}

// Rocket normally represents routes as attributed functions. The generator
// emits every function and preserves order in batched routes! mounts.

struct RocketScalingResponse(&'static str);

impl<'r> rocket::response::Responder<'r, 'static> for RocketScalingResponse {
    fn respond_to(self, _request: &'r rocket::Request<'_>) -> rocket::response::Result<'static> {
        rocket::Response::build()
            .status(rocket::http::Status::Ok)
            .raw_header(MARKER_HEADER, self.0)
            .sized_body(self.0.len(), Cursor::new(self.0))
            .ok()
    }
}

struct RocketEmpty(rocket::http::Status);

impl<'r> rocket::response::Responder<'r, 'static> for RocketEmpty {
    fn respond_to(self, _request: &'r rocket::Request<'_>) -> rocket::response::Result<'static> {
        rocket::Response::build().status(self.0).ok()
    }
}

macro_rules! define_rocket_route {
    ($name:ident, $path:literal, $response:literal) => {
        #[rocket::get($path)]
        fn $name() -> RocketScalingResponse {
            RocketScalingResponse($response)
        }
    };
}

#[rocket::catch(404)]
fn rocket_scaling_not_found() -> RocketEmpty {
    RocketEmpty(rocket::http::Status::NotFound)
}

fn rocket_scaling_application() -> rocket::Rocket<rocket::Build> {
    rocket::custom(
        rocket::Config::figment().merge(("log_level", rocket::config::LogLevel::Off)),
    )
}

include!("../generated/http_dispatch_scaling.rs");

struct Fixture {
    size: RouteSetSize,
    framework: Framework,
    factory: CallFactory,
}

struct Fixtures {
    fixtures: Vec<Fixture>,
}

impl Fixtures {
    fn new_checked() -> Self {
        let mut fixtures = Vec::with_capacity(RouteSetSize::ALL.len() * Framework::ALL.len());
        for size in RouteSetSize::ALL {
            for framework in Framework::ALL {
                fixtures.push(Fixture {
                    size,
                    framework,
                    factory: build_factory(size, framework),
                });
            }
        }
        let fixtures = Self { fixtures };
        fixtures.assert_equivalent();
        fixtures
    }

    fn prepare(
        &self,
        size: RouteSetSize,
        framework: Framework,
        scenario: Scenario,
    ) -> PreparedCall {
        let fixture = self
            .fixtures
            .iter()
            .find(|fixture| fixture.size == size && fixture.framework == framework)
            .expect("every generated size/framework pair has exactly one fixture");
        (fixture.factory)(scenario)
    }

    fn assert_equivalent(&self) {
        for size in RouteSetSize::ALL {
            for scenario in Scenario::ALL {
                let expected = scenario.expected(size);
                for framework in Framework::ALL {
                    let actual = self.prepare(size, framework, scenario)();
                    assert_eq!(
                        actual,
                        expected,
                        "{} produced a different {size:?}/{scenario:?} response",
                        framework.name(),
                    );
                }
            }
        }
    }
}

fn build_factory(size: RouteSetSize, framework: Framework) -> CallFactory {
    match framework {
        Framework::Routerama => match size {
            RouteSetSize::Routes16 => build_routerama_16_factory(),
            RouteSetSize::Routes128 => build_routerama_128_factory(),
            RouteSetSize::Routes1024 => build_routerama_1024_factory(),
        },
        Framework::Axum => build_axum_factory(size),
        Framework::ActixWeb => build_actix_web_factory(size),
        Framework::Rocket => build_rocket_factory(size),
        Framework::Warp => build_warp_factory(size),
    }
}

// Axum registers the generated table in order in its normal runtime Router.

async fn axum_not_found() -> impl axum::response::IntoResponse {
    (axum::http::StatusCode::NOT_FOUND, "")
}

fn build_axum_router(size: RouteSetSize) -> axum::Router {
    use axum::routing::get;

    let mut router = axum::Router::new();
    for route in size.routes() {
        let response = route.response;
        router = router.route(
            route.path,
            get(move || async move {
                (
                    axum::http::StatusCode::OK,
                    [(MARKER_HEADER, response)],
                    response,
                )
            }),
        );
    }
    router.fallback(axum_not_found).with_state(())
}

fn build_axum_factory(size: RouteSetSize) -> CallFactory {
    let runtime = new_runtime();
    let router = process_lifetime(RefCell::new(build_axum_router(size)));
    Box::new(move |scenario| {
        let request = http::Request::builder()
            .method("GET")
            .uri(scenario.path(size))
            .body(axum::body::Body::empty())
            .expect("the generated benchmark request metadata is valid");
        Box::new(move || {
            let mut router = router.borrow_mut();
            run_on_runtime(runtime, async move {
                let response = TowerService::call(&mut *router, request)
                    .await
                    .expect("the Axum router is infallible");
                let status = response.status().as_u16();
                let marker = response
                    .headers()
                    .get(MARKER_HEADER)
                    .map(|value| Fingerprint::of(value.as_bytes()));
                response_observation(status, marker, response.into_body()).await
            })
        })
    })
}

// Actix Web registers the generated table in order on its normal App builder.

async fn actix_not_found() -> actix_web::HttpResponse {
    actix_web::HttpResponse::NotFound().finish()
}

fn build_actix_web_factory(size: RouteSetSize) -> CallFactory {
    use actix_web::{App, test, web};

    let runtime = new_runtime();
    let mut application = App::new();
    for route in size.routes() {
        let response = route.response;
        application = application.route(
            route.path,
            web::get().to(move || async move {
                actix_web::HttpResponse::Ok()
                    .insert_header((MARKER_HEADER, response))
                    .body(response)
            }),
        );
    }
    let service = run_on_runtime(
        runtime,
        test::init_service(application.default_service(web::to(actix_not_found))),
    );
    let service = process_lifetime(service);

    Box::new(move |scenario| {
        let request = test::TestRequest::get()
            .uri(scenario.path(size))
            .to_request();
        Box::new(move || {
            run_on_runtime(runtime, async move {
                let response = test::call_service(service, request).await;
                let status = response.status().as_u16();
                let marker = response
                    .headers()
                    .get(MARKER_HEADER)
                    .map(|value| Fingerprint::of(value.as_bytes()));
                let body = actix_web::body::to_bytes(response.into_body())
                    .await
                    .expect("the Actix Web benchmark response body is infallible");
                Observation {
                    status,
                    marker,
                    body: Fingerprint::of(&body),
                }
            })
        })
    })
}

fn build_rocket_factory(size: RouteSetSize) -> CallFactory {
    use rocket::local::asynchronous::Client;

    let runtime = new_runtime();
    let application = match size {
        RouteSetSize::Routes16 => rocket_routes_16(),
        RouteSetSize::Routes128 => rocket_routes_128(),
        RouteSetSize::Routes1024 => rocket_routes_1024(),
    };
    let client = run_on_runtime(runtime, Client::untracked(application))
        .expect("the generated Rocket benchmark application ignites");
    let client = process_lifetime(client);

    Box::new(move |scenario| {
        let request = client.get(scenario.path(size));
        Box::new(move || {
            run_on_runtime(runtime, async move {
                let response = request.dispatch().await;
                let status = response.status().code;
                let marker = response
                    .headers()
                    .get_one(MARKER_HEADER)
                    .map(|value| Fingerprint::of(value.as_bytes()));
                let body = response.into_bytes().await.unwrap_or_default();
                Observation {
                    status,
                    marker,
                    body: Fingerprint::of(&body),
                }
            })
        })
    })
}

// Warp has no mutable route registry. Its normal representation is a composed
// `or` filter; boxing every leaf and branch keeps 1,024 routes compilable. A
// balanced tree retains left-to-right `or` semantics without a 1,024-frame
// call stack on the last and miss paths.

type WarpRoutes = warp::filters::BoxedFilter<(warp::reply::Response,)>;

fn warp_response(
    status: warp::http::StatusCode,
    marker: Option<&'static str>,
    body: &'static str,
) -> warp::reply::Response {
    let response = warp::reply::with_status(body, status);
    match marker {
        Some(value) => {
            warp::reply::with_header(response, MARKER_HEADER, value).into_response()
        }
        None => response.into_response(),
    }
}

fn warp_route(route: RouteSpec) -> WarpRoutes {
    warp::get()
        .and(warp::path("scale"))
        .and(warp::path(route.set_segment))
        .and(warp::path(route.route_segment))
        .and(warp::path::end())
        .map(move || {
            warp_response(
                warp::http::StatusCode::OK,
                Some(route.response),
                route.response,
            )
        })
        .boxed()
}

fn warp_route_tree(routes: &[RouteSpec]) -> WarpRoutes {
    match routes {
        [] => unreachable!("the generator emits nonempty route sets"),
        [route] => warp_route(*route),
        _ => {
            let middle = routes.len() / 2;
            let (left, right) = routes.split_at(middle);
            warp_route_tree(left)
                .or(warp_route_tree(right))
                .unify()
                .boxed()
        }
    }
}

fn build_warp_routes(size: RouteSetSize) -> WarpRoutes {
    let routes = warp_route_tree(size.routes());
    routes
        .or(
            warp::any()
                .map(|| warp_response(warp::http::StatusCode::NOT_FOUND, None, ""))
                .boxed(),
        )
        .unify()
        .boxed()
}

fn build_warp_factory(size: RouteSetSize) -> CallFactory {
    let runtime = new_runtime();
    let service = process_lifetime(RefCell::new(warp::service(build_warp_routes(size))));
    Box::new(move |scenario| {
        let request = http::Request::builder()
            .method("GET")
            .uri(scenario.path(size))
            .body(Empty::<Bytes>::new())
            .expect("the generated benchmark request metadata is valid");
        Box::new(move || {
            let mut service = service.borrow_mut();
            run_on_runtime(runtime, async move {
                let response = TowerService::call(&mut *service, request)
                    .await
                    .unwrap_or_else(|error: Infallible| match error {});
                let status = response.status().as_u16();
                let marker = response
                    .headers()
                    .get(MARKER_HEADER)
                    .map(|value| Fingerprint::of(value.as_bytes()));
                response_observation(status, marker, response.into_body()).await
            })
        })
    })
}

fn setup_prepared(
    size: RouteSetSize,
    framework: Framework,
    scenario: Scenario,
) -> PreparedCall {
    let factory = build_factory(size, framework);
    let expected = scenario.expected(size);
    let warm = factory(scenario)();
    assert_eq!(
        warm,
        expected,
        "{} produced a different {size:?}/{scenario:?} warmup response",
        framework.name(),
    );
    factory(scenario)
}
