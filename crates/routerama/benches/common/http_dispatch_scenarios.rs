// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared, network-free HTTP routing and dispatch fixtures. Every framework
// receives the same 16-route application, request metadata, response payloads,
// and rejection policy. Framework and runtime construction, request creation,
// and one complete equivalence/warmup sweep happen outside measured calls.

use std::convert::Infallible;
use std::cell::RefCell;
use std::future::Future;
use std::hint::black_box;
use std::io::Cursor;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};
use tower_service::Service as TowerService;
use warp::{Filter as _, Reply as _};

const MARKER_HEADER: &str = "x-fixture";
const MARKER_VALUE: &str = "ready";

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
    LiteralFirst,
    LiteralMiddle,
    LiteralLast,
    Captures,
    MethodHeaderQuery,
    ResponseStatusHeader,
    CompleteMiss,
    CaptureConversionFailure,
}

impl Scenario {
    const ALL: [Self; 8] = [
        Self::LiteralFirst,
        Self::LiteralMiddle,
        Self::LiteralLast,
        Self::Captures,
        Self::MethodHeaderQuery,
        Self::ResponseStatusHeader,
        Self::CompleteMiss,
        Self::CaptureConversionFailure,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::LiteralFirst => "literal_first",
            Self::LiteralMiddle => "literal_middle",
            Self::LiteralLast => "literal_last",
            Self::Captures => "captures",
            Self::MethodHeaderQuery => "method_header_query",
            Self::ResponseStatusHeader => "response_status_header",
            Self::CompleteMiss => "complete_miss",
            Self::CaptureConversionFailure => "capture_conversion_failure",
        }
    }

    const fn method(self) -> &'static str {
        if matches!(self, Self::MethodHeaderQuery) {
            "POST"
        } else {
            "GET"
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::LiteralFirst => "/literal/first",
            Self::LiteralMiddle => "/literal/middle",
            Self::LiteralLast => "/literal/last",
            Self::Captures => "/capture/alice/42",
            Self::MethodHeaderQuery => "/extract?q=routerama&page=2",
            Self::ResponseStatusHeader => "/response",
            Self::CompleteMiss => "/missing",
            Self::CaptureConversionFailure => "/capture/alice/not-a-number",
        }
    }

    const fn request_header(self) -> Option<(&'static str, &'static str)> {
        if matches!(self, Self::MethodHeaderQuery) {
            Some(("x-mode", "fast"))
        } else {
            None
        }
    }

    fn expected(self) -> Observation {
        match self {
            Self::LiteralFirst => Observation::new(200, None, b"first"),
            Self::LiteralMiddle => Observation::new(200, None, b"middle"),
            Self::LiteralLast => Observation::new(200, None, b"last"),
            Self::Captures => Observation::new(200, None, b"alice:42"),
            Self::MethodHeaderQuery => Observation::new(200, None, b"POST:fast:routerama:2"),
            Self::ResponseStatusHeader => Observation::new(201, Some(MARKER_VALUE.as_bytes()), b"created"),
            Self::CompleteMiss => Observation::new(404, None, b""),
            Self::CaptureConversionFailure => Observation::new(400, None, b""),
        }
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

struct Fixtures {
    routerama: CallFactory,
    axum: CallFactory,
    actix_web: CallFactory,
    rocket: CallFactory,
    warp: CallFactory,
}

impl Fixtures {
    fn new_checked() -> Self {
        let fixtures = Self {
            routerama: build_routerama_factory(),
            axum: build_axum_factory(),
            actix_web: build_actix_web_factory(),
            rocket: build_rocket_factory(),
            warp: build_warp_factory(),
        };
        fixtures.assert_equivalent();
        fixtures
    }

    fn prepare(&self, framework: Framework, scenario: Scenario) -> PreparedCall {
        let factory = match framework {
            Framework::Routerama => &self.routerama,
            Framework::Axum => &self.axum,
            Framework::ActixWeb => &self.actix_web,
            Framework::Rocket => &self.rocket,
            Framework::Warp => &self.warp,
        };
        factory(scenario)
    }

    fn assert_equivalent(&self) {
        for scenario in Scenario::ALL {
            let expected = scenario.expected();
            for framework in Framework::ALL {
                let actual = self.prepare(framework, scenario)();
                assert_eq!(
                    actual,
                    expected,
                    "{} produced a different {:?} response",
                    framework.name(),
                    scenario
                );
            }
        }
    }

    fn record_allocation_sweeps(&self) -> [u64; 5] {
        let session = alloc_tracker::Session::new().no_file();
        let mut allocated_bytes = [0; 5];
        for (index, framework) in Framework::ALL.into_iter().enumerate() {
            let calls: Vec<_> = Scenario::ALL
                .into_iter()
                .map(|scenario| self.prepare(framework, scenario))
                .collect();
            let operation = session.operation(framework.name());
            {
                let _span = operation.measure_thread();
                for call in calls {
                    black_box(call());
                }
            }
            allocated_bytes[index] = operation.total_bytes_allocated();
        }
        allocated_bytes
    }
}

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

#[derive(Debug, Deserialize, routerama::query::FromQuery)]
struct SearchQuery {
    q: String,
    page: u32,
}

// Routerama.

struct RouteramaFixture;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl RouteramaFixture {
    #[route(GET, "/literal/first")]
    async fn literal_first(&self) -> &'static str {
        "first"
    }

    #[route(GET, "/fixture/01")]
    async fn filler_01(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/capture/{name}/{id}")]
    async fn captures(&self, name: &str, id: u32) -> String {
        format!("{name}:{id}")
    }

    #[route(GET, "/fixture/03")]
    async fn filler_03(&self) -> &'static str {
        "filler"
    }

    #[route(POST, "/extract")]
    async fn extract(
        &self,
        method: routerama::route::Method,
        headers: routerama::route::HeaderMap,
        query: routerama::route::Query<SearchQuery>,
    ) -> String {
        let mode = headers
            .get("x-mode")
            .and_then(|value| value.to_str().ok())
            .expect("the fixture supplies a valid x-mode header");
        format!("{method}:{mode}:{}:{}", query.q, query.page)
    }

    #[route(GET, "/fixture/05")]
    async fn filler_05(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/response")]
    async fn response(
        &self,
    ) -> (
        routerama::route::StatusCode,
        [(http::HeaderName, http::HeaderValue); 1],
        &'static str,
    ) {
        (
            routerama::route::StatusCode::CREATED,
            [(
                http::HeaderName::from_static(MARKER_HEADER),
                http::HeaderValue::from_static(MARKER_VALUE),
            )],
            "created",
        )
    }

    #[route(GET, "/fixture/07")]
    async fn filler_07(&self) -> &'static str {
        "filler"
    }

    #[route(GET, "/literal/middle")]
    async fn literal_middle(&self) -> &'static str {
        "middle"
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

    #[route(GET, "/literal/last")]
    async fn literal_last(&self) -> &'static str {
        "last"
    }
}

fn build_routerama_factory() -> CallFactory {
    let runtime = new_runtime();
    let fixture = process_lifetime(RouteramaFixture);
    Box::new(move |scenario| {
        let mut request = http::Request::builder()
            .method(scenario.method())
            .uri(scenario.path());
        if let Some((name, value)) = scenario.request_header() {
            request = request.header(name, value);
        }
        let request = request.body(()).expect("the benchmark request metadata is valid");
        Box::new(move || {
            run_on_runtime(runtime, async move {
                let response = fixture.route(request, &()).await;
                let status = response.status().as_u16();
                let marker = response.headers().get(MARKER_HEADER);
                let marker = marker.map(|value| Fingerprint::of(value.as_bytes()));
                response_observation(status, marker, response.into_body()).await
            })
        })
    })
}

// Axum.

async fn axum_literal_first() -> &'static str {
    "first"
}

async fn axum_literal_middle() -> &'static str {
    "middle"
}

async fn axum_literal_last() -> &'static str {
    "last"
}

async fn axum_filler() -> &'static str {
    "filler"
}

async fn axum_captures(
    captures: Result<
        axum::extract::Path<(String, u32)>,
        axum::extract::rejection::PathRejection,
    >,
) -> axum::response::Response {
    match captures {
        Ok(axum::extract::Path((name, id))) => {
            axum::response::IntoResponse::into_response(format!("{name}:{id}"))
        }
        Err(_) => axum::response::IntoResponse::into_response((axum::http::StatusCode::BAD_REQUEST, "")),
    }
}

async fn axum_extract(
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<SearchQuery>,
) -> String {
    let mode = headers
        .get("x-mode")
        .and_then(|value| value.to_str().ok())
        .expect("the fixture supplies a valid x-mode header");
    format!("{method}:{mode}:{}:{}", query.q, query.page)
}

async fn axum_response() -> impl axum::response::IntoResponse {
    (
        axum::http::StatusCode::CREATED,
        [(MARKER_HEADER, MARKER_VALUE)],
        "created",
    )
}

async fn axum_not_found() -> impl axum::response::IntoResponse {
    (axum::http::StatusCode::NOT_FOUND, "")
}

fn build_axum_router() -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/literal/first", get(axum_literal_first))
        .route("/fixture/01", get(axum_filler))
        .route("/capture/{name}/{id}", get(axum_captures))
        .route("/fixture/03", get(axum_filler))
        .route("/extract", post(axum_extract))
        .route("/fixture/05", get(axum_filler))
        .route("/response", get(axum_response))
        .route("/fixture/07", get(axum_filler))
        .route("/literal/middle", get(axum_literal_middle))
        .route("/fixture/09", get(axum_filler))
        .route("/fixture/10", get(axum_filler))
        .route("/fixture/11", get(axum_filler))
        .route("/fixture/12", get(axum_filler))
        .route("/fixture/13", get(axum_filler))
        .route("/fixture/14", get(axum_filler))
        .route("/literal/last", get(axum_literal_last))
        .fallback(axum_not_found)
        .with_state(())
}

fn build_axum_factory() -> CallFactory {
    let runtime = new_runtime();
    let router = process_lifetime(RefCell::new(build_axum_router()));
    Box::new(move |scenario| {
        let mut request = http::Request::builder()
            .method(scenario.method())
            .uri(scenario.path());
        if let Some((name, value)) = scenario.request_header() {
            request = request.header(name, value);
        }
        let request = request
            .body(axum::body::Body::empty())
            .expect("the benchmark request metadata is valid");
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

// Actix Web.

async fn actix_literal_first() -> &'static str {
    "first"
}

async fn actix_literal_middle() -> &'static str {
    "middle"
}

async fn actix_literal_last() -> &'static str {
    "last"
}

async fn actix_filler() -> &'static str {
    "filler"
}

#[expect(
    clippy::future_not_send,
    reason = "Actix Web request handlers are polled by its local service and need not be Send"
)]
async fn actix_captures(
    path: Result<actix_web::web::Path<(String, u32)>, actix_web::Error>,
) -> actix_web::HttpResponse {
    match path {
        Ok(path) => actix_web::HttpResponse::Ok().body(format!("{}:{}", path.0, path.1)),
        Err(_) => actix_web::HttpResponse::BadRequest().finish(),
    }
}

#[expect(
    clippy::future_not_send,
    reason = "Actix Web request handlers are polled by its local service and need not be Send"
)]
async fn actix_extract(
    request: actix_web::HttpRequest,
    query: actix_web::web::Query<SearchQuery>,
) -> String {
    let mode = request
        .headers()
        .get("x-mode")
        .and_then(|value| value.to_str().ok())
        .expect("the fixture supplies a valid x-mode header");
    format!("{}:{mode}:{}:{}", request.method(), query.q, query.page)
}

async fn actix_response() -> actix_web::HttpResponse {
    actix_web::HttpResponse::Created()
        .insert_header((MARKER_HEADER, MARKER_VALUE))
        .body("created")
}

async fn actix_not_found() -> actix_web::HttpResponse {
    actix_web::HttpResponse::NotFound().finish()
}

fn build_actix_web_factory() -> CallFactory {
    use actix_web::{App, test, web};

    let runtime = new_runtime();
    let service = run_on_runtime(
        runtime,
        test::init_service(
            App::new()
                .route("/literal/first", web::get().to(actix_literal_first))
                .route("/fixture/01", web::get().to(actix_filler))
                .route("/capture/{name}/{id}", web::get().to(actix_captures))
                .route("/fixture/03", web::get().to(actix_filler))
                .route("/extract", web::post().to(actix_extract))
                .route("/fixture/05", web::get().to(actix_filler))
                .route("/response", web::get().to(actix_response))
                .route("/fixture/07", web::get().to(actix_filler))
                .route("/literal/middle", web::get().to(actix_literal_middle))
                .route("/fixture/09", web::get().to(actix_filler))
                .route("/fixture/10", web::get().to(actix_filler))
                .route("/fixture/11", web::get().to(actix_filler))
                .route("/fixture/12", web::get().to(actix_filler))
                .route("/fixture/13", web::get().to(actix_filler))
                .route("/fixture/14", web::get().to(actix_filler))
                .route("/literal/last", web::get().to(actix_literal_last))
                .default_service(web::to(actix_not_found)),
        ),
    );
    let service = process_lifetime(service);

    Box::new(move |scenario| {
        let method = scenario
            .method()
            .parse()
            .expect("the benchmark request method is valid");
        let mut request = test::TestRequest::default()
            .method(method)
            .uri(scenario.path());
        if let Some((name, value)) = scenario.request_header() {
            request = request.insert_header((name, value));
        }
        let request = request.to_request();
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

// Rocket.

struct RocketMetadata<'r> {
    method: rocket::http::Method,
    mode: &'r str,
}

#[derive(rocket::FromForm)]
struct RocketSearchQuery {
    q: String,
    page: u32,
}

#[rocket::async_trait]
impl<'r> rocket::request::FromRequest<'r> for RocketMetadata<'r> {
    type Error = ();

    async fn from_request(request: &'r rocket::Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        let Some(mode) = request.headers().get_one("x-mode") else {
            return rocket::request::Outcome::Error((rocket::http::Status::BadRequest, ()));
        };
        rocket::request::Outcome::Success(Self {
            method: request.method(),
            mode,
        })
    }
}

struct RocketEmpty(rocket::http::Status);

impl<'r> rocket::response::Responder<'r, 'static> for RocketEmpty {
    fn respond_to(self, _request: &'r rocket::Request<'_>) -> rocket::response::Result<'static> {
        rocket::Response::build().status(self.0).ok()
    }
}

struct RocketCreated;

impl<'r> rocket::response::Responder<'r, 'static> for RocketCreated {
    fn respond_to(self, _request: &'r rocket::Request<'_>) -> rocket::response::Result<'static> {
        rocket::Response::build()
            .status(rocket::http::Status::Created)
            .raw_header(MARKER_HEADER, MARKER_VALUE)
            .sized_body(7, Cursor::new("created"))
            .ok()
    }
}

#[rocket::get("/literal/first")]
fn rocket_literal_first() -> &'static str {
    "first"
}

#[rocket::get("/literal/middle")]
fn rocket_literal_middle() -> &'static str {
    "middle"
}

#[rocket::get("/literal/last")]
fn rocket_literal_last() -> &'static str {
    "last"
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
rocket_filler!(rocket_filler_03, "/fixture/03");
rocket_filler!(rocket_filler_05, "/fixture/05");
rocket_filler!(rocket_filler_07, "/fixture/07");
rocket_filler!(rocket_filler_09, "/fixture/09");
rocket_filler!(rocket_filler_10, "/fixture/10");
rocket_filler!(rocket_filler_11, "/fixture/11");
rocket_filler!(rocket_filler_12, "/fixture/12");
rocket_filler!(rocket_filler_13, "/fixture/13");
rocket_filler!(rocket_filler_14, "/fixture/14");

#[rocket::get("/capture/<name>/<id>")]
fn rocket_captures(name: &str, id: &str) -> Result<String, RocketEmpty> {
    id.parse::<u32>()
        .map(|id| format!("{name}:{id}"))
        .map_err(|_error| RocketEmpty(rocket::http::Status::BadRequest))
}

#[rocket::post("/extract?<query..>")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Rocket request guards and form values are injected into route handlers by value"
)]
fn rocket_extract(metadata: RocketMetadata<'_>, query: RocketSearchQuery) -> String {
    let RocketMetadata { method, mode } = metadata;
    let RocketSearchQuery { q, page } = query;
    format!("{method}:{mode}:{q}:{page}")
}

#[rocket::get("/response")]
fn rocket_response() -> RocketCreated {
    RocketCreated
}

#[rocket::catch(404)]
fn rocket_not_found() -> RocketEmpty {
    RocketEmpty(rocket::http::Status::NotFound)
}

#[expect(
    clippy::redundant_type_annotations,
    reason = "Rocket's routes and catchers macros emit explicit internal types"
)]
fn build_rocket_factory() -> CallFactory {
    use rocket::local::asynchronous::Client;

    let runtime = new_runtime();
    let rocket = rocket::custom(
        rocket::Config::figment().merge(("log_level", rocket::config::LogLevel::Off)),
    )
    .mount(
        "/",
        rocket::routes![
            rocket_literal_first,
            rocket_filler_01,
            rocket_captures,
            rocket_filler_03,
            rocket_extract,
            rocket_filler_05,
            rocket_response,
            rocket_filler_07,
            rocket_literal_middle,
            rocket_filler_09,
            rocket_filler_10,
            rocket_filler_11,
            rocket_filler_12,
            rocket_filler_13,
            rocket_filler_14,
            rocket_literal_last,
        ],
    )
    .register("/", rocket::catchers![rocket_not_found]);
    let client = run_on_runtime(
        runtime,
        Client::untracked(rocket),
    )
    .expect("the Rocket benchmark application ignites");
    let client = process_lifetime(client);

    Box::new(move |scenario| {
        let mut request = if matches!(scenario, Scenario::MethodHeaderQuery) {
            client.post(scenario.path())
        } else {
            client.get(scenario.path())
        };
        if let Some((name, value)) = scenario.request_header() {
            request = request.header(rocket::http::Header::new(name, value));
        }
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

// Warp.

type WarpRoutes = warp::filters::BoxedFilter<(warp::reply::Response,)>;

fn warp_response(status: warp::http::StatusCode, marker: Option<&'static str>, body: &'static str) -> warp::reply::Response {
    let response = warp::reply::with_status(body, status);
    match marker {
        Some(value) => warp::reply::with_header(response, MARKER_HEADER, value).into_response(),
        None => response.into_response(),
    }
}

fn warp_literal(segment: &'static str, body: &'static str) -> WarpRoutes {
    warp::get()
        .and(warp::path("literal"))
        .and(warp::path(segment))
        .and(warp::path::end())
        .map(move || warp_response(warp::http::StatusCode::OK, None, body))
        .boxed()
}

fn warp_filler(segment: &'static str) -> WarpRoutes {
    warp::get()
        .and(warp::path("fixture"))
        .and(warp::path(segment))
        .and(warp::path::end())
        .map(|| warp_response(warp::http::StatusCode::OK, None, "filler"))
        .boxed()
}

fn warp_captures() -> WarpRoutes {
    let typed = warp::get()
        .and(warp::path("capture"))
        .and(warp::path::param::<String>())
        .and(warp::path::param::<u32>())
        .and(warp::path::end())
        .map(|name: String, id: u32| {
            let body = format!("{name}:{id}");
            warp::reply::with_status(body, warp::http::StatusCode::OK).into_response()
        })
        .boxed();
    let invalid = warp::get()
        .and(warp::path("capture"))
        .and(warp::path::param::<String>())
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .map(|_name: String, _id: String| warp_response(warp::http::StatusCode::BAD_REQUEST, None, ""))
        .boxed();
    typed.or(invalid).unify().boxed()
}

fn warp_extract() -> WarpRoutes {
    warp::post()
        .and(warp::path("extract"))
        .and(warp::path::end())
        .and(warp::method())
        .and(warp::header::<String>("x-mode"))
        .and(warp::query::<SearchQuery>())
        .map(
            |method: warp::http::Method, mode: String, query: SearchQuery| {
                let body = format!("{method}:{mode}:{}:{}", query.q, query.page);
                warp::reply::with_status(body, warp::http::StatusCode::OK).into_response()
            },
        )
        .boxed()
}

fn warp_created() -> WarpRoutes {
    warp::get()
        .and(warp::path("response"))
        .and(warp::path::end())
        .map(|| {
            warp_response(
                warp::http::StatusCode::CREATED,
                Some(MARKER_VALUE),
                "created",
            )
        })
        .boxed()
}

fn warp_or(left: WarpRoutes, right: WarpRoutes) -> WarpRoutes {
    left.or(right).unify().boxed()
}

fn build_warp_routes() -> WarpRoutes {
    let routes = warp_literal("first", "first");
    let routes = warp_or(routes, warp_filler("01"));
    let routes = warp_or(routes, warp_captures());
    let routes = warp_or(routes, warp_filler("03"));
    let routes = warp_or(routes, warp_extract());
    let routes = warp_or(routes, warp_filler("05"));
    let routes = warp_or(routes, warp_created());
    let routes = warp_or(routes, warp_filler("07"));
    let routes = warp_or(routes, warp_literal("middle", "middle"));
    let routes = warp_or(routes, warp_filler("09"));
    let routes = warp_or(routes, warp_filler("10"));
    let routes = warp_or(routes, warp_filler("11"));
    let routes = warp_or(routes, warp_filler("12"));
    let routes = warp_or(routes, warp_filler("13"));
    let routes = warp_or(routes, warp_filler("14"));
    let routes = warp_or(routes, warp_literal("last", "last"));
    warp_or(
        routes,
        warp::any()
            .map(|| warp_response(warp::http::StatusCode::NOT_FOUND, None, ""))
            .boxed(),
    )
}

fn build_warp_factory() -> CallFactory {
    let runtime = new_runtime();
    let service = process_lifetime(RefCell::new(warp::service(build_warp_routes())));
    Box::new(move |scenario| {
        let mut request = http::Request::builder()
            .method(scenario.method())
            .uri(scenario.path());
        if let Some((name, value)) = scenario.request_header() {
            request = request.header(name, value);
        }
        let request = request
            .body(Empty::<Bytes>::new())
            .expect("the benchmark request metadata is valid");
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

fn setup_prepared(framework: Framework, scenario: Scenario) -> PreparedCall {
    Fixtures::new_checked().prepare(framework, scenario)
}
