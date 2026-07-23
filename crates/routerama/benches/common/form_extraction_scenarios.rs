// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Equivalent bounded form extraction for each framework. Measured work covers
// buffering through complete response observation; setup remains outside.
// `docs/PERF.md` records exclusions and normalized rejection statuses.

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
const FORM_MEDIA_TYPE: &str = "application/x-www-form-urlencoded";
const ABSENT_NOTE: &str = "-";
const FORM_PATH: &str = "/form/registration";

const FORM_SINGLE: &str = "name=Ada+Lovelace&count=7&note=first";
const FORM_SPLIT_FIRST: &str = "name=Ada+Lovelace&cou";
const FORM_SPLIT_SECOND: &str = "nt=7&note=first";
const FORM_PERCENT: &str = "name=Ada%20Lovelace&count=7&note=caf%C3%A9";
const FORM_OPTIONAL_ABSENT: &str = "name=Ada+Lovelace&count=7";
const FORM_AT_LIMIT: &str = "name=Ada&count=7&note=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
const FORM_OVER_LIMIT: &str = "name=Ada&count=7&note=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
const FORM_INVALID_NUMBER: &str = "name=Ada+Lovelace&count=seven";
const FORM_MISSING_FIELD: &str = "count=7&note=first";
const AT_LIMIT_NOTE: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

const _: () = assert!(FORM_AT_LIMIT.len() == BODY_LIMIT);
const _: () = assert!(FORM_OVER_LIMIT.len() == BODY_LIMIT + 1);
const _: () = assert!(FORM_SINGLE.len() < BODY_LIMIT);
const _: () = assert!(FORM_PERCENT.len() < BODY_LIMIT);

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
    SingleSuccess,
    SplitSuccess,
    AtLimitSuccess,
    PercentEncodedSuccess,
    OptionalAbsentSuccess,
    OverLimit,
    InvalidNumber,
    MissingField,
    UnsupportedContentType,
    MissingContentType,
}

impl Scenario {
    const ALL: [Self; 10] = [
        Self::SingleSuccess,
        Self::SplitSuccess,
        Self::AtLimitSuccess,
        Self::PercentEncodedSuccess,
        Self::OptionalAbsentSuccess,
        Self::OverLimit,
        Self::InvalidNumber,
        Self::MissingField,
        Self::UnsupportedContentType,
        Self::MissingContentType,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::SingleSuccess => "form_single_success",
            Self::SplitSuccess => "form_split_success",
            Self::AtLimitSuccess => "form_64_success",
            Self::PercentEncodedSuccess => "form_percent_encoded_success",
            Self::OptionalAbsentSuccess => "form_optional_absent_success",
            Self::OverLimit => "form_encoded_65_rejected",
            Self::InvalidNumber => "form_invalid_number",
            Self::MissingField => "form_missing_field",
            Self::UnsupportedContentType => "unsupported_form_content_type",
            Self::MissingContentType => "missing_form_content_type",
        }
    }

    const fn payload(self) -> PayloadSpec {
        match self {
            Self::SingleSuccess | Self::UnsupportedContentType | Self::MissingContentType => {
                PayloadSpec::one(FORM_SINGLE.as_bytes())
            }
            Self::SplitSuccess => PayloadSpec::two(FORM_SPLIT_FIRST.as_bytes(), FORM_SPLIT_SECOND.as_bytes()),
            Self::AtLimitSuccess => PayloadSpec::one(FORM_AT_LIMIT.as_bytes()),
            Self::PercentEncodedSuccess => PayloadSpec::one(FORM_PERCENT.as_bytes()),
            Self::OptionalAbsentSuccess => PayloadSpec::one(FORM_OPTIONAL_ABSENT.as_bytes()),
            Self::OverLimit => PayloadSpec::one(FORM_OVER_LIMIT.as_bytes()),
            Self::InvalidNumber => PayloadSpec::one(FORM_INVALID_NUMBER.as_bytes()),
            Self::MissingField => PayloadSpec::one(FORM_MISSING_FIELD.as_bytes()),
        }
    }

    const fn content_type(self) -> Option<&'static str> {
        match self {
            Self::SingleSuccess
            | Self::SplitSuccess
            | Self::AtLimitSuccess
            | Self::PercentEncodedSuccess
            | Self::OptionalAbsentSuccess
            | Self::OverLimit
            | Self::InvalidNumber
            | Self::MissingField => Some(FORM_MEDIA_TYPE),
            Self::UnsupportedContentType => Some("text/plain; charset=utf-8"),
            Self::MissingContentType => None,
        }
    }

    fn expected(self) -> Observation {
        match self {
            Self::SingleSuccess | Self::SplitSuccess => success("Ada Lovelace", 7, "first"),
            Self::AtLimitSuccess => success("Ada", 7, AT_LIMIT_NOTE),
            Self::PercentEncodedSuccess => success("Ada Lovelace", 7, "caf\u{e9}"),
            Self::OptionalAbsentSuccess => success("Ada Lovelace", 7, ABSENT_NOTE),
            Self::OverLimit => Observation::new(413, None, b""),
            Self::InvalidNumber | Self::MissingField => Observation::new(400, None, b""),
            Self::UnsupportedContentType | Self::MissingContentType => Observation::new(415, None, b""),
        }
    }
}

fn rendered(name: &str, count: u32, note: &str) -> String {
    format!("{name}:{count}:{note}")
}

fn success(name: &str, count: u32, note: &str) -> Observation {
    Observation::new(200, Some(MARKER_VALUE.as_bytes()), rendered(name, count, note).as_bytes())
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

/// The Routerama schema, decoded by the crate's own `FromQuery` codec.
#[derive(Debug, routerama::query::FromQuery, PartialEq, Eq)]
struct Registration {
    name: String,
    count: u32,
    note: Option<String>,
}

/// The identical Serde schema used by Axum, Actix Web, and Warp. A missing
/// `Option` field deserializes to `None` in Serde exactly as it does in
/// Routerama's codec, so no `#[serde(default)]` annotation is required.
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct SerdeRegistration {
    name: String,
    count: u32,
    note: Option<String>,
}

/// The identical Rocket schema, decoded by Rocket's own form parser.
#[derive(Debug, rocket::FromForm, PartialEq, Eq)]
struct RocketRegistration {
    name: String,
    count: u32,
    note: Option<String>,
}

fn render_serde(registration: &SerdeRegistration) -> String {
    rendered(
        &registration.name,
        registration.count,
        registration.note.as_deref().unwrap_or(ABSENT_NOTE),
    )
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
        assert_eq!(
            FORM_AT_LIMIT.len(),
            BODY_LIMIT,
            "the inclusive form fixture must remain exactly at the limit"
        );
        assert_eq!(
            FORM_OVER_LIMIT.len(),
            BODY_LIMIT + 1,
            "the rejected form fixture must remain exactly one encoded byte over the limit"
        );
        assert_eq!(
            FORM_SPLIT_FIRST.len() + FORM_SPLIT_SECOND.len(),
            FORM_SINGLE.len(),
            "the split form fixture must encode the same bytes as the single-frame fixture"
        );
        let decoded: SerdeRegistration =
            serde_urlencoded::from_str(FORM_AT_LIMIT).expect("the at-limit fixture must remain a valid encoded form");
        assert_eq!(decoded.note.as_deref(), Some(AT_LIMIT_NOTE), "the at-limit note must not drift");
        let decoded: SerdeRegistration =
            serde_urlencoded::from_str(FORM_OVER_LIMIT).expect("the over-limit fixture must remain a valid encoded form");
        assert_eq!(
            decoded.note.map(|note| note.len()),
            Some(AT_LIMIT_NOTE.len() + 1),
            "the over-limit fixture must differ from the at-limit fixture by exactly one byte"
        );
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

struct RouteramaFormFixture;

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
impl RouteramaFormFixture {
    #[route(POST, "/form/registration")]
    async fn register(
        &self,
        #[body] form: routerama::route::form::Form<Registration, BODY_LIMIT>,
    ) -> (
        routerama::route::StatusCode,
        [(http::HeaderName, http::HeaderValue); 1],
        String,
    ) {
        let registration = form.into_inner();
        (
            routerama::route::StatusCode::OK,
            routerama_marker(),
            rendered(
                &registration.name,
                registration.count,
                registration.note.as_deref().unwrap_or(ABSENT_NOTE),
            ),
        )
    }
}

fn build_routerama_factory() -> CallFactory {
    let runtime = new_runtime();
    let fixture = process_lifetime(RouteramaFormFixture);
    Box::new(move |scenario| {
        let request = build_http_request(scenario, PayloadSpec::body);
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

fn build_http_request<B>(scenario: Scenario, body: impl FnOnce(PayloadSpec) -> B) -> http::Request<B> {
    let payload = scenario.payload();
    let mut request = http::Request::builder()
        .method("POST")
        .uri(FORM_PATH)
        .header(http::header::CONTENT_LENGTH, payload.len().to_string());
    if let Some(content_type) = scenario.content_type() {
        request = request.header(http::header::CONTENT_TYPE, content_type);
    }
    request
        .body(body(payload))
        .expect("the benchmark request metadata is valid")
}

// Axum.

fn axum_success(body: String) -> axum::response::Response {
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

/// Axum reports a form decode failure as 422. The fixture policy is 400, so the
/// application maps its own typed rejection instead of forwarding the default.
fn axum_form_error_status(rejection: &axum::extract::rejection::FormRejection) -> axum::http::StatusCode {
    use axum::extract::rejection::FormRejection;

    match rejection {
        FormRejection::InvalidFormContentType(_) => axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        FormRejection::FailedToDeserializeForm(_) | FormRejection::FailedToDeserializeFormBody(_) => {
            axum::http::StatusCode::BAD_REQUEST
        }
        other => other.status(),
    }
}

async fn axum_form(
    form: Result<axum::Form<SerdeRegistration>, axum::extract::rejection::FormRejection>,
) -> axum::response::Response {
    match form {
        Ok(axum::Form(registration)) => axum_success(render_serde(&registration)),
        Err(rejection) => axum_failure(axum_form_error_status(&rejection)),
    }
}

fn build_axum_router() -> axum::Router {
    use axum::routing::post;

    axum::Router::new()
        .route(FORM_PATH, post(axum_form))
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(())
}

fn build_axum_factory() -> CallFactory {
    let runtime = new_runtime();
    let router = process_lifetime(RefCell::new(build_axum_router()));
    Box::new(move |scenario| {
        let request = build_http_request(scenario, |payload| axum::body::Body::new(payload.body()));
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
async fn actix_form(form: Result<actix_web::web::Form<SerdeRegistration>, actix_web::Error>) -> actix_web::HttpResponse {
    match form {
        Ok(form) => actix_web::HttpResponse::Ok()
            .insert_header((MARKER_HEADER, MARKER_VALUE))
            .body(render_serde(&form.into_inner())),
        Err(error) => actix_web::HttpResponse::build(actix_form_error_status(&error)).finish(),
    }
}

/// Actix Web's own `UrlencodedError` statuses already match the fixture policy;
/// the mapping stays explicit so a future upstream change is visible here.
fn actix_form_error_status(error: &actix_web::Error) -> actix_web::http::StatusCode {
    use actix_web::error::UrlencodedError;

    match error.as_error::<UrlencodedError>() {
        Some(UrlencodedError::ContentType) => actix_web::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Some(UrlencodedError::Overflow { .. }) => actix_web::http::StatusCode::PAYLOAD_TOO_LARGE,
        Some(_) => actix_web::http::StatusCode::BAD_REQUEST,
        None => error.as_response_error().status_code(),
    }
}

fn build_actix_web_factory() -> CallFactory {
    use actix_web::{App, test, web};

    let runtime = new_runtime();
    let service = run_on_runtime(
        runtime,
        test::init_service(
            App::new()
                .app_data(web::FormConfig::default().limit(BODY_LIMIT))
                .route(FORM_PATH, web::post().to(actix_form)),
        ),
    );
    let service = process_lifetime(service);

    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = test::TestRequest::post()
            .uri(FORM_PATH)
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
enum RocketFormError<'r> {
    UnsupportedMediaType,
    Native(rocket::form::Errors<'r>),
}

struct RocketFormBody(RocketRegistration);

#[rocket::async_trait]
impl<'r> rocket::data::FromData<'r> for RocketFormBody {
    type Error = RocketFormError<'r>;

    async fn from_data(req: &'r rocket::Request<'_>, data: rocket::Data<'r>) -> rocket::data::Outcome<'r, Self> {
        use rocket::outcome::Outcome;

        if !req
            .content_type()
            .is_some_and(|content_type| content_type.media_type().is_form())
        {
            return Outcome::Error((
                rocket::http::Status::UnsupportedMediaType,
                RocketFormError::UnsupportedMediaType,
            ));
        }

        match <rocket::form::Form<RocketRegistration> as rocket::data::FromData<'r>>::from_data(req, data).await {
            Outcome::Success(form) => Outcome::Success(Self(form.into_inner())),
            Outcome::Error((status, error)) => Outcome::Error((status, RocketFormError::Native(error))),
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

/// Rocket reports form validation failures as 422. The fixture policy is 400,
/// so only its explicit payload-too-large status is forwarded unchanged.
fn rocket_form_error_status(error: &RocketFormError<'_>) -> rocket::http::Status {
    match error {
        RocketFormError::UnsupportedMediaType => rocket::http::Status::UnsupportedMediaType,
        RocketFormError::Native(errors) => {
            if errors.status() == rocket::http::Status::PayloadTooLarge {
                rocket::http::Status::PayloadTooLarge
            } else {
                rocket::http::Status::BadRequest
            }
        }
    }
}

#[rocket::post("/form/registration", data = "<form>")]
fn rocket_form(form: Result<RocketFormBody, RocketFormError<'_>>) -> RocketFixtureResponse {
    match form {
        Ok(RocketFormBody(registration)) => RocketFixtureResponse::success(
            rendered(
                &registration.name,
                registration.count,
                registration.note.as_deref().unwrap_or(ABSENT_NOTE),
            )
            .into_bytes(),
        ),
        Err(error) => RocketFixtureResponse::failure(rocket_form_error_status(&error)),
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
    let limits = Limits::new().limit("form", limit);
    let rocket = rocket::custom(rocket::Config {
        log_level: rocket::config::LogLevel::Off,
        limits,
        ..rocket::Config::debug_default()
    })
    .mount("/", rocket::routes![rocket_form]);
    let client = run_on_runtime(runtime, Client::untracked(rocket)).expect("the Rocket benchmark application ignites");
    let client = process_lifetime(client);

    Box::new(move |scenario| {
        let payload = scenario.payload();
        let mut request = client
            .post(FORM_PATH)
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
struct WarpMissingFormContentType;

impl warp::reject::Reject for WarpMissingFormContentType {}

fn warp_required_form_content_type() -> impl warp::Filter<Extract = (), Error = warp::Rejection> + Copy {
    warp::header::optional::<warp::http::HeaderValue>("content-type")
        .and_then(|content_type: Option<warp::http::HeaderValue>| async move {
            if content_type.is_some() {
                Ok(())
            } else {
                Err(warp::reject::custom(WarpMissingFormContentType))
            }
        })
        .untuple_one()
}

fn warp_success(body: String) -> warp::reply::Response {
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
    warp::post()
        .and(warp::path("form"))
        .and(warp::path("registration"))
        .and(warp::path::end())
        // Warp's form filter accepts a missing Content-Type. Require presence
        // here, then let the native filter validate the supplied media type.
        .and(warp_required_form_content_type())
        .and(warp::body::content_length_limit(limit))
        .and(warp::body::form::<SerdeRegistration>())
        .map(|registration: SerdeRegistration| warp_success(render_serde(&registration)))
        .recover(|rejection: warp::Rejection| async move {
            let status = if rejection.find::<warp::reject::PayloadTooLarge>().is_some() {
                warp::http::StatusCode::PAYLOAD_TOO_LARGE
            } else if rejection.find::<warp::reject::UnsupportedMediaType>().is_some()
                || rejection.find::<WarpMissingFormContentType>().is_some()
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
        // Warp's normal limit checks Content-Length only. The Limited adapter
        // also caps bytes actually yielded before form() can buffer.
        let request = build_http_request(scenario, |payload| http_body_util::Limited::new(payload.body(), BODY_LIMIT));
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
