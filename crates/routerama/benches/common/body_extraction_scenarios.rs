// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared, network-free bounded-body extraction fixtures. Every framework
// receives the same encoded bytes, 64-byte limit, response payloads, and
// application-level rejection policy. Runtime/application construction,
// request and payload creation, and equivalence checks stay outside measured
// calls; buffering, decoding, handler work, and complete response observation
// stay inside.

use std::cell::RefCell;
use std::convert::Infallible;
use std::future::Future;
use std::hint::black_box;
use std::io::Cursor;
use std::pin::Pin;
use std::task::{Context, Poll};

use alloc_tracker::Session;
use bytes::Bytes;
use http_body::{Body as _, Frame, SizeHint};
use http_body_util::BodyExt as _;
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};
use tower_service::Service as TowerService;
use warp::{Filter as _, Reply as _};

const BODY_LIMIT: usize = 64;

fn total_bytes_allocated(session: &Session, operation_name: &str) -> u64 {
    let missing_operation = format!(
        "operation {operation_name:?} must match a name registered with Session::operation on this session"
    );

    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == operation_name).then(|| operation.total_bytes_allocated()))
        .expect(&missing_operation)
}

const MARKER_HEADER: &str = "x-fixture";
const MARKER_VALUE: &str = "ready";
const BYTES_AT_LIMIT: [u8; BODY_LIMIT] = [b'x'; BODY_LIMIT];
const BYTES_OVER_LIMIT: [u8; BODY_LIMIT + 1] = [b'x'; BODY_LIMIT + 1];
const TEXT_OVER_LIMIT: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
const JSON_OVER_LIMIT: &[u8] = br#"{"name":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","count":7}"#;

const _: () = assert!(BYTES_AT_LIMIT.len() == BODY_LIMIT);
const _: () = assert!(BYTES_OVER_LIMIT.len() == BODY_LIMIT + 1);
const _: () = assert!(TEXT_OVER_LIMIT.len() == BODY_LIMIT + 1);
const _: () = assert!(JSON_OVER_LIMIT.len() == BODY_LIMIT + 1);

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
    BytesSingleSuccess,
    BytesSplitSuccess,
    BytesAtLimitSuccess,
    TextSuccess,
    JsonSuccess,
    BytesOverLimit,
    TextOverLimit,
    JsonOverLimit,
    InvalidUtf8,
    MalformedJson,
    UnsupportedJsonContentType,
    MissingJsonContentType,
}

impl Scenario {
    const ALL: [Self; 12] = [
        Self::BytesSingleSuccess,
        Self::BytesSplitSuccess,
        Self::BytesAtLimitSuccess,
        Self::TextSuccess,
        Self::JsonSuccess,
        Self::BytesOverLimit,
        Self::TextOverLimit,
        Self::JsonOverLimit,
        Self::InvalidUtf8,
        Self::MalformedJson,
        Self::UnsupportedJsonContentType,
        Self::MissingJsonContentType,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::BytesSingleSuccess => "bytes_single_success",
            Self::BytesSplitSuccess => "bytes_split_success",
            Self::BytesAtLimitSuccess => "bytes_64_success",
            Self::TextSuccess => "text_success",
            Self::JsonSuccess => "json_success",
            Self::BytesOverLimit => "bytes_65_rejected",
            Self::TextOverLimit => "text_utf8_65_rejected",
            Self::JsonOverLimit => "json_encoded_65_rejected",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::MalformedJson => "malformed_json",
            Self::UnsupportedJsonContentType => "unsupported_json_content_type",
            Self::MissingJsonContentType => "missing_json_content_type",
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::BytesSingleSuccess | Self::BytesSplitSuccess | Self::BytesAtLimitSuccess | Self::BytesOverLimit => {
                "/body/bytes"
            }
            Self::TextSuccess | Self::TextOverLimit | Self::InvalidUtf8 => "/body/text",
            Self::JsonSuccess
            | Self::JsonOverLimit
            | Self::MalformedJson
            | Self::UnsupportedJsonContentType
            | Self::MissingJsonContentType => "/body/json",
        }
    }

    const fn payload(self) -> PayloadSpec {
        match self {
            Self::BytesSingleSuccess => PayloadSpec::one(b"split-body"),
            Self::BytesSplitSuccess => PayloadSpec::two(b"split-", b"body"),
            Self::BytesAtLimitSuccess => PayloadSpec::one(&BYTES_AT_LIMIT),
            Self::TextSuccess => PayloadSpec::one("bounded UTF-8: \u{2713}".as_bytes()),
            Self::JsonSuccess | Self::UnsupportedJsonContentType | Self::MissingJsonContentType => {
                PayloadSpec::one(br#"{"name":"Ada","count":7}"#)
            }
            Self::BytesOverLimit => PayloadSpec::one(&BYTES_OVER_LIMIT),
            Self::TextOverLimit => PayloadSpec::one(TEXT_OVER_LIMIT.as_bytes()),
            Self::JsonOverLimit => PayloadSpec::one(JSON_OVER_LIMIT),
            Self::InvalidUtf8 => PayloadSpec::one(b"text-\xff"),
            Self::MalformedJson => PayloadSpec::one(br#"{"name":,"count":7}"#),
        }
    }

    const fn content_type(self) -> Option<&'static str> {
        match self {
            Self::BytesSingleSuccess | Self::BytesSplitSuccess | Self::BytesAtLimitSuccess | Self::BytesOverLimit => {
                Some("application/octet-stream")
            }
            Self::TextSuccess | Self::TextOverLimit | Self::InvalidUtf8 | Self::UnsupportedJsonContentType => {
                Some("text/plain; charset=utf-8")
            }
            Self::JsonSuccess | Self::JsonOverLimit | Self::MalformedJson => Some("application/json"),
            Self::MissingJsonContentType => None,
        }
    }

    fn expected(self) -> Observation {
        match self {
            Self::BytesSingleSuccess | Self::BytesSplitSuccess => {
                Observation::new(200, Some(MARKER_VALUE.as_bytes()), b"split-body")
            }
            Self::BytesAtLimitSuccess => Observation::new(200, Some(MARKER_VALUE.as_bytes()), &BYTES_AT_LIMIT),
            Self::TextSuccess => Observation::new(200, Some(MARKER_VALUE.as_bytes()), "bounded UTF-8: \u{2713}".as_bytes()),
            Self::JsonSuccess => Observation::new(200, Some(MARKER_VALUE.as_bytes()), b"Ada:7"),
            Self::BytesOverLimit | Self::TextOverLimit | Self::JsonOverLimit => Observation::new(413, None, b""),
            Self::InvalidUtf8 | Self::MalformedJson => Observation::new(400, None, b""),
            Self::UnsupportedJsonContentType | Self::MissingJsonContentType => Observation::new(415, None, b""),
        }
    }
}

#[derive(Clone, Copy)]
struct PayloadSpec {
    first: &'static [u8],
    second: Option<&'static [u8]>,
}

impl PayloadSpec {
    const fn one(first: &'static [u8]) -> Self {
        Self { first, second: None }
    }

    const fn two(first: &'static [u8], second: &'static [u8]) -> Self {
        Self {
            first,
            second: Some(second),
        }
    }

    fn len(self) -> usize {
        self.first.len() + self.second.map_or(0, <[u8]>::len)
    }

    fn body(self) -> FixtureBody {
        FixtureBody {
            first: Some(Bytes::from_static(self.first)),
            second: self.second.map(Bytes::from_static),
        }
    }

    fn contiguous(self) -> Bytes {
        let Some(second) = self.second else {
            return Bytes::from_static(self.first);
        };

        let mut bytes = Vec::with_capacity(self.len());
        bytes.extend_from_slice(self.first);
        bytes.extend_from_slice(second);
        Bytes::from(bytes)
    }
}

#[derive(Debug)]
struct FixtureBody {
    first: Option<Bytes>,
    second: Option<Bytes>,
}

impl http_body::Body for FixtureBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(
            self.first
                .take()
                .or_else(|| self.second.take())
                .map(|bytes| Ok(Frame::data(bytes))),
        )
    }

    fn is_end_stream(&self) -> bool {
        self.first.is_none() && self.second.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        let length = self.first.as_ref().map_or(0, Bytes::len) + self.second.as_ref().map_or(0, Bytes::len);
        SizeHint::with_exact(u64::try_from(length).expect("fixture bodies are at most 65 bytes and always fit in u64"))
    }
}

struct ActixPayloadStream {
    body: FixtureBody,
}

impl futures_core::Stream for ActixPayloadStream {
    type Item = Result<Bytes, actix_web::error::PayloadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.body).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                let data = frame
                    .into_data()
                    .expect("fixture request bodies contain data frames only");
                Poll::Ready(Some(Ok(data)))
            }
            Poll::Ready(Some(Err(error))) => match error {},
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct JsonPayload {
    name: String,
    count: u32,
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
        Self::assert_limit_payloads();
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

    fn assert_limit_payloads() {
        assert_eq!(BYTES_AT_LIMIT.len(), BODY_LIMIT, "the inclusive bytes fixture must remain exactly at the limit");
        assert_eq!(
            BYTES_OVER_LIMIT.len(),
            BODY_LIMIT + 1,
            "the rejected bytes fixture must remain exactly one byte over the limit"
        );
        assert_eq!(
            TEXT_OVER_LIMIT.len(),
            BODY_LIMIT + 1,
            "the rejected UTF-8 fixture must remain exactly one encoded byte over the limit"
        );
        assert_eq!(
            JSON_OVER_LIMIT.len(),
            BODY_LIMIT + 1,
            "the rejected JSON fixture must remain exactly one encoded byte over the limit"
        );
        let decoded: JsonPayload =
            serde_json::from_slice(JSON_OVER_LIMIT).expect("the over-limit JSON fixture must remain valid JSON");
        assert_eq!(decoded.name.len(), 44, "the over-limit JSON fixture name must not drift");
        assert_eq!(decoded.count, 7, "the over-limit JSON fixture count must not drift");
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
                    "{} produced a different {scenario:?} response",
                    framework.name()
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
                // This diagnostic spans extraction through complete response
                // observation. It is intentionally not the handler-entry gate.
                let _span = operation.measure_thread().iterations(Scenario::ALL.len() as u64);
                for call in calls {
                    black_box(call());
                }
            }
            allocated_bytes[index] = total_bytes_allocated(&session, framework.name());
        }
        allocated_bytes
    }
}

fn process_lifetime<T: 'static>(value: T) -> &'static T {
    // Retain every runtime and initialized application so no measured request
    // can drop the final owning reference and perform teardown.
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

fn marker(headers: &http::HeaderMap) -> Option<Fingerprint> {
    headers
        .get(MARKER_HEADER)
        .map(|value| Fingerprint::of(value.as_bytes()))
}

// Routerama.

struct RouteramaBodyFixture;

fn routerama_marker() -> [(http::HeaderName, http::HeaderValue); 1] {
    [(
        http::HeaderName::from_static(MARKER_HEADER),
        http::HeaderValue::from_static(MARKER_VALUE),
    )]
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl RouteramaBodyFixture {
    #[route(POST, "/body/bytes")]
    async fn bytes(
        &self,
        #[body] body: routerama::route::BytesBody<BODY_LIMIT>,
    ) -> (
        routerama::route::StatusCode,
        [(http::HeaderName, http::HeaderValue); 1],
        Bytes,
    ) {
        (
            routerama::route::StatusCode::OK,
            routerama_marker(),
            body.into_inner(),
        )
    }

    #[route(POST, "/body/text")]
    async fn text(
        &self,
        #[body] body: routerama::route::TextBody<BODY_LIMIT>,
    ) -> (
        routerama::route::StatusCode,
        [(http::HeaderName, http::HeaderValue); 1],
        String,
    ) {
        (
            routerama::route::StatusCode::OK,
            routerama_marker(),
            body.into_inner(),
        )
    }

    #[route(POST, "/body/json")]
    async fn json(
        &self,
        #[body] body: routerama::route::json::Json<JsonPayload, BODY_LIMIT>,
    ) -> (
        routerama::route::StatusCode,
        [(http::HeaderName, http::HeaderValue); 1],
        String,
    ) {
        let payload = body.into_inner();
        (
            routerama::route::StatusCode::OK,
            routerama_marker(),
            format!("{}:{}", payload.name, payload.count),
        )
    }
}

fn build_routerama_factory() -> CallFactory {
    let runtime = new_runtime();
    let fixture = process_lifetime(RouteramaBodyFixture);
    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = http::Request::builder()
            .method("POST")
            .uri(scenario.path())
            .header(http::header::CONTENT_LENGTH, payload.len().to_string());
        if let Some(content_type) = scenario.content_type() {
            request = request.header(http::header::CONTENT_TYPE, content_type);
        }
        let request = request.body(payload.body()).expect("the Routerama benchmark request metadata is valid");
        Box::new(move || {
            run_on_runtime(runtime, async move {
                let response = fixture.route(request, &()).await;
                let status = response.status().as_u16();
                let marker = marker(response.headers());
                response_observation(status, marker, response.into_body()).await
            })
        })
    })
}

// Axum.

fn axum_success<T>(body: T) -> axum::response::Response
where
    axum::body::Body: From<T>,
{
    http::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(MARKER_HEADER, MARKER_VALUE)
        .body(axum::body::Body::from(body))
        .expect("the static Axum response metadata is valid")
}

fn axum_failure(status: axum::http::StatusCode) -> axum::response::Response {
    http::Response::builder()
        .status(status)
        .body(axum::body::Body::empty())
        .expect("the static Axum response metadata is valid")
}

async fn axum_bytes(body: Result<Bytes, axum::extract::rejection::BytesRejection>) -> axum::response::Response {
    match body {
        Ok(body) => axum_success(body),
        Err(rejection) => axum_failure(rejection.status()),
    }
}

async fn axum_text(body: Result<String, axum::extract::rejection::StringRejection>) -> axum::response::Response {
    match body {
        Ok(body) => axum_success(body),
        Err(rejection) => axum_failure(rejection.status()),
    }
}

async fn axum_json(
    body: Result<axum::Json<JsonPayload>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    match body {
        Ok(axum::Json(payload)) => axum_success(format!("{}:{}", payload.name, payload.count)),
        Err(rejection) => axum_failure(rejection.status()),
    }
}

fn build_axum_router() -> axum::Router {
    use axum::routing::post;

    axum::Router::new()
        .route("/body/bytes", post(axum_bytes))
        .route("/body/text", post(axum_text))
        .route("/body/json", post(axum_json))
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(())
}

fn build_axum_factory() -> CallFactory {
    let runtime = new_runtime();
    let router = process_lifetime(RefCell::new(build_axum_router()));
    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = http::Request::builder()
            .method("POST")
            .uri(scenario.path())
            .header(http::header::CONTENT_LENGTH, payload.len().to_string());
        if let Some(content_type) = scenario.content_type() {
            request = request.header(http::header::CONTENT_TYPE, content_type);
        }
        let request = request
            .body(axum::body::Body::new(payload.body()))
            .expect("the Axum benchmark request metadata is valid");
        Box::new(move || {
            let mut router = router.borrow_mut();
            run_on_runtime(runtime, async move {
                let response = TowerService::call(&mut *router, request)
                    .await
                    .expect("the Axum router is infallible");
                let status = response.status().as_u16();
                let marker = marker(response.headers());
                response_observation(status, marker, response.into_body()).await
            })
        })
    })
}

// Actix Web.

#[expect(
    clippy::future_not_send,
    reason = "Actix Web handler futures and extraction errors are intentionally local"
)]
async fn actix_bytes(body: Result<actix_web::web::Bytes, actix_web::Error>) -> actix_web::HttpResponse {
    match body {
        Ok(body) => actix_web::HttpResponse::Ok()
            .insert_header((MARKER_HEADER, MARKER_VALUE))
            .body(body),
        Err(error) => actix_web::HttpResponse::build(error.as_response_error().status_code()).finish(),
    }
}

#[expect(
    clippy::future_not_send,
    reason = "Actix Web handler futures and extraction errors are intentionally local"
)]
async fn actix_text(body: Result<String, actix_web::Error>) -> actix_web::HttpResponse {
    match body {
        Ok(body) => actix_web::HttpResponse::Ok()
            .insert_header((MARKER_HEADER, MARKER_VALUE))
            .body(body),
        Err(error) => actix_web::HttpResponse::build(error.as_response_error().status_code()).finish(),
    }
}

fn actix_json_error_status(error: &actix_web::Error) -> actix_web::http::StatusCode {
    use actix_web::error::JsonPayloadError;

    match error.as_error::<JsonPayloadError>() {
        Some(JsonPayloadError::ContentType) => actix_web::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Some(JsonPayloadError::OverflowKnownLength { .. } | JsonPayloadError::Overflow { .. }) => {
            actix_web::http::StatusCode::PAYLOAD_TOO_LARGE
        }
        Some(_) => actix_web::http::StatusCode::BAD_REQUEST,
        None => error.as_response_error().status_code(),
    }
}

#[expect(
    clippy::future_not_send,
    reason = "Actix Web handler futures and extraction errors are intentionally local"
)]
async fn actix_json(body: Result<actix_web::web::Json<JsonPayload>, actix_web::Error>) -> actix_web::HttpResponse {
    match body {
        Ok(body) => {
            let payload = body.into_inner();
            actix_web::HttpResponse::Ok()
                .insert_header((MARKER_HEADER, MARKER_VALUE))
                .body(format!("{}:{}", payload.name, payload.count))
        }
        Err(error) => actix_web::HttpResponse::build(actix_json_error_status(&error)).finish(),
    }
}

fn build_actix_web_factory() -> CallFactory {
    use actix_web::{App, test, web};

    let runtime = new_runtime();
    let service = run_on_runtime(
        runtime,
        test::init_service(
            App::new()
                .app_data(web::PayloadConfig::new(BODY_LIMIT))
                .app_data(web::JsonConfig::default().limit(BODY_LIMIT))
                .route("/body/bytes", web::post().to(actix_bytes))
                .route("/body/text", web::post().to(actix_text))
                .route("/body/json", web::post().to(actix_json)),
        ),
    );
    let service = process_lifetime(service);

    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = test::TestRequest::post()
            .uri(scenario.path())
            .insert_header((actix_web::http::header::CONTENT_LENGTH, payload.len().to_string()));
        if let Some(content_type) = scenario.content_type() {
            request = request.insert_header((actix_web::http::header::CONTENT_TYPE, content_type));
        }
        let request = request.to_request();
        let stream: Pin<Box<dyn futures_core::Stream<Item = Result<Bytes, actix_web::error::PayloadError>>>> =
            Box::pin(ActixPayloadStream { body: payload.body() });
        let request = request.replace_payload(actix_web::dev::Payload::from(stream)).0;
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

#[derive(Debug)]
enum RocketJsonError<'r> {
    UnsupportedMediaType,
    Native(rocket::serde::json::Error<'r>),
}

struct RocketJsonBody(JsonPayload);

#[rocket::async_trait]
impl<'r> rocket::data::FromData<'r> for RocketJsonBody {
    type Error = RocketJsonError<'r>;

    async fn from_data(req: &'r rocket::Request<'_>, data: rocket::Data<'r>) -> rocket::data::Outcome<'r, Self> {
        use rocket::outcome::Outcome;

        if !req
            .content_type()
            .is_some_and(|content_type| content_type.media_type().is_json())
        {
            return Outcome::Error((
                rocket::http::Status::UnsupportedMediaType,
                RocketJsonError::UnsupportedMediaType,
            ));
        }

        match <rocket::serde::json::Json<JsonPayload> as rocket::data::FromData<'r>>::from_data(req, data).await {
            Outcome::Success(rocket::serde::json::Json(payload)) => Outcome::Success(Self(payload)),
            Outcome::Error((status, error)) => Outcome::Error((status, RocketJsonError::Native(error))),
            Outcome::Forward(forward) => Outcome::Forward(forward),
        }
    }
}

struct RocketFixtureResponse {
    status: rocket::http::Status,
    marker: bool,
    body: Vec<u8>,
}

impl RocketFixtureResponse {
    fn success(body: Vec<u8>) -> Self {
        Self {
            status: rocket::http::Status::Ok,
            marker: true,
            body,
        }
    }

    const fn failure(status: rocket::http::Status) -> Self {
        Self {
            status,
            marker: false,
            body: Vec::new(),
        }
    }
}

impl<'r> rocket::response::Responder<'r, 'static> for RocketFixtureResponse {
    fn respond_to(self, _request: &'r rocket::Request<'_>) -> rocket::response::Result<'static> {
        let mut response = rocket::Response::build();
        response.status(self.status);
        if self.marker {
            response.raw_header(MARKER_HEADER, MARKER_VALUE);
        }
        response.sized_body(self.body.len(), Cursor::new(self.body)).ok()
    }
}

fn rocket_io_status(error: &std::io::Error) -> rocket::http::Status {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        rocket::http::Status::PayloadTooLarge
    } else {
        rocket::http::Status::BadRequest
    }
}

#[rocket::post("/body/bytes", data = "<body>")]
fn rocket_bytes(body: Result<Vec<u8>, std::io::Error>) -> RocketFixtureResponse {
    match body {
        Ok(body) => RocketFixtureResponse::success(body),
        Err(error) => RocketFixtureResponse::failure(rocket_io_status(&error)),
    }
}

#[rocket::post("/body/text", data = "<body>")]
fn rocket_text(body: Result<String, std::io::Error>) -> RocketFixtureResponse {
    match body {
        Ok(body) => RocketFixtureResponse::success(body.into_bytes()),
        Err(error) => RocketFixtureResponse::failure(rocket_io_status(&error)),
    }
}

fn rocket_json_error_status(error: &RocketJsonError<'_>) -> rocket::http::Status {
    match error {
        RocketJsonError::UnsupportedMediaType => rocket::http::Status::UnsupportedMediaType,
        RocketJsonError::Native(rocket::serde::json::Error::Io(error))
            if error.kind() == std::io::ErrorKind::UnexpectedEof =>
        {
            rocket::http::Status::PayloadTooLarge
        }
        RocketJsonError::Native(_) => rocket::http::Status::BadRequest,
    }
}

#[rocket::post("/body/json", data = "<body>")]
fn rocket_json(body: Result<RocketJsonBody, RocketJsonError<'_>>) -> RocketFixtureResponse {
    match body {
        Ok(RocketJsonBody(payload)) => RocketFixtureResponse::success(format!("{}:{}", payload.name, payload.count).into_bytes()),
        Err(error) => RocketFixtureResponse::failure(rocket_json_error_status(&error)),
    }
}

#[expect(
    clippy::redundant_type_annotations,
    reason = "Rocket's routes macro emits explicit internal types"
)]
fn build_rocket_factory() -> CallFactory {
    use rocket::data::{Limits, ToByteUnit as _};
    use rocket::local::asynchronous::Client;

    let runtime = new_runtime();
    let limit = u64::try_from(BODY_LIMIT)
        .expect("the 64-byte fixture limit always fits in u64")
        .bytes();
    let limits = Limits::new()
        .limit("bytes", limit)
        .limit("string", limit)
        .limit("json", limit);
    let rocket = rocket::custom(rocket::Config {
        log_level: rocket::config::LogLevel::Off,
        limits,
        ..rocket::Config::debug_default()
    })
    .mount("/", rocket::routes![rocket_bytes, rocket_text, rocket_json]);
    let client = run_on_runtime(runtime, Client::untracked(rocket)).expect("the Rocket benchmark application ignites");
    let client = process_lifetime(client);

    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = client
            .post(scenario.path())
            .header(rocket::http::Header::new("content-length", payload.len().to_string()));
        if let Some(content_type) = scenario.content_type() {
            request = request.header(rocket::http::Header::new("content-type", content_type));
        }
        let request = request.body(payload.contiguous());
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

#[derive(Debug)]
struct WarpMissingJsonContentType;

impl warp::reject::Reject for WarpMissingJsonContentType {}

fn warp_required_json_content_type() -> impl warp::Filter<Extract = (), Error = warp::Rejection> + Copy {
    warp::header::optional::<warp::http::HeaderValue>("content-type")
        .and_then(|content_type: Option<warp::http::HeaderValue>| async move {
            if content_type.is_some() {
                Ok(())
            } else {
                Err(warp::reject::custom(WarpMissingJsonContentType))
            }
        })
        .untuple_one()
}

fn warp_success<T>(body: T) -> warp::reply::Response
where
    http::Response<T>: warp::Reply,
{
    http::Response::builder()
        .status(warp::http::StatusCode::OK)
        .header(MARKER_HEADER, MARKER_VALUE)
        .body(body)
        .expect("the static Warp response metadata is valid")
        .into_response()
}

fn warp_failure(status: warp::http::StatusCode) -> warp::reply::Response {
    http::Response::builder()
        .status(status)
        .body(Vec::<u8>::new())
        .expect("the static Warp response metadata is valid")
        .into_response()
}

fn build_warp_routes() -> WarpRoutes {
    let limit = u64::try_from(BODY_LIMIT).expect("the 64-byte fixture limit always fits in u64");
    let bytes = warp::post()
        .and(warp::path("body"))
        .and(warp::path("bytes"))
        .and(warp::path::end())
        .and(warp::body::content_length_limit(limit))
        .and(warp::body::bytes())
        .map(warp_success);
    let text = warp::post()
        .and(warp::path("body"))
        .and(warp::path("text"))
        .and(warp::path::end())
        .and(warp::body::content_length_limit(limit))
        .and(warp::body::bytes())
        .map(|bytes: Bytes| match core::str::from_utf8(&bytes) {
            Ok(text) => warp_success(text.to_owned()),
            Err(_) => warp_failure(warp::http::StatusCode::BAD_REQUEST),
        });
    let json = warp::post()
        .and(warp::path("body"))
        .and(warp::path("json"))
        .and(warp::path::end())
        // Warp's JSON filter accepts a missing Content-Type. Require presence
        // here, then let the native filter validate the supplied media type.
        .and(warp_required_json_content_type())
        .and(warp::body::content_length_limit(limit))
        .and(warp::body::json::<JsonPayload>())
        .map(|payload: JsonPayload| warp_success(format!("{}:{}", payload.name, payload.count)));

    bytes
        .or(text)
        .unify()
        .or(json)
        .unify()
        .recover(|rejection: warp::Rejection| async move {
            let status = if rejection.find::<warp::reject::PayloadTooLarge>().is_some() {
                warp::http::StatusCode::PAYLOAD_TOO_LARGE
            } else if rejection.find::<warp::reject::UnsupportedMediaType>().is_some()
                || rejection.find::<WarpMissingJsonContentType>().is_some()
            {
                warp::http::StatusCode::UNSUPPORTED_MEDIA_TYPE
            } else if rejection.is_not_found() {
                warp::http::StatusCode::NOT_FOUND
            } else {
                warp::http::StatusCode::BAD_REQUEST
            };
            Ok::<_, Infallible>(warp_failure(status))
        })
        .unify()
        .boxed()
}

fn build_warp_factory() -> CallFactory {
    let runtime = new_runtime();
    let service = process_lifetime(RefCell::new(warp::service(build_warp_routes())));
    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = http::Request::builder()
            .method("POST")
            .uri(scenario.path())
            .header(http::header::CONTENT_LENGTH, payload.len().to_string());
        if let Some(content_type) = scenario.content_type() {
            request = request.header(http::header::CONTENT_TYPE, content_type);
        }
        // Warp's normal limit checks Content-Length only. The Limited adapter
        // also caps bytes actually yielded before bytes()/json() can buffer.
        let body = http_body_util::Limited::new(payload.body(), BODY_LIMIT);
        let request = request.body(body).expect("the Warp benchmark request metadata is valid");
        Box::new(move || {
            let mut service = service.borrow_mut();
            run_on_runtime(runtime, async move {
                let response = TowerService::call(&mut *service, request)
                    .await
                    .unwrap_or_else(|error: Infallible| match error {});
                let status = response.status().as_u16();
                let marker = marker(response.headers());
                response_observation(status, marker, response.into_body()).await
            })
        })
    })
}

fn setup_prepared(framework: Framework, scenario: Scenario) -> PreparedCall {
    Fixtures::new_checked().prepare(framework, scenario)
}
