// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared, network-free end-to-end Routerama rejection fixtures. Router and
// request construction happen outside measured calls; measured work starts at
// `Router::route` and ends after observing the fully constructed HTTP
// response, covering the whole path a real server pays for a rejection:
// method/path matching (or extractor buffering and decoding), failure
// classification, and response construction.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::{Request, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use serde::Deserialize;

const JSON_LIMIT: usize = 64;
const JSON_UNDER_LIMIT: &[u8] = br#"{"name":"Ada","count":7}"#;
const JSON_OVER_LIMIT: &[u8] = br#"{"name":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","count":7}"#;
const JSON_MALFORMED: &[u8] = br#"{"name":,"count":7}"#;

const _: () = assert!(JSON_UNDER_LIMIT.len() <= JSON_LIMIT);
const _: () = assert!(JSON_OVER_LIMIT.len() > JSON_LIMIT);
const _: () = assert!(JSON_MALFORMED.len() <= JSON_LIMIT);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct JsonPayload {
    name: String,
    count: u32,
}

#[derive(Debug)]
struct FixtureBody(Option<Bytes>);

impl FixtureBody {
    const fn empty() -> Self {
        Self(None)
    }

    fn from_static(bytes: &'static [u8]) -> Self {
        Self(Some(Bytes::from_static(bytes)))
    }
}

impl HttpBody for FixtureBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(mut self: std::pin::Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.0.take().map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.0.as_ref().map_or(0, Bytes::len) as u64)
    }
}

struct RejectionFixture;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[routerama::route::router]
impl RejectionFixture {
    #[route(GET, "/health")]
    async fn health(&self) -> &'static str {
        "ok"
    }

    #[route(POST, "/submit")]
    async fn submit(&self) -> &'static str {
        "submitted"
    }

    #[route(POST, "/body/json")]
    async fn json(&self, #[body] body: routerama::route::json::Json<JsonPayload, JSON_LIMIT>) -> String {
        let payload = body.into_inner();
        format!("{}:{}", payload.name, payload.count)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Success,
    NotFound,
    // Routerama's resolver matches method and path together and reports a
    // mismatch on either as the same `RouteFailure::NotFound`; there is no
    // distinct 405 status. A request for a registered path with the wrong
    // method therefore observes 404, same as an unregistered path.
    MethodNotAllowed,
    PayloadTooLarge,
    MalformedJson,
}

impl Scenario {
    const ALL: [Self; 5] = [
        Self::Success,
        Self::NotFound,
        Self::MethodNotAllowed,
        Self::PayloadTooLarge,
        Self::MalformedJson,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::PayloadTooLarge => "payload_too_large",
            Self::MalformedJson => "malformed_json",
        }
    }

    const fn method(self) -> &'static str {
        match self {
            Self::Success | Self::NotFound | Self::MethodNotAllowed => "GET",
            Self::PayloadTooLarge | Self::MalformedJson => "POST",
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::Success => "/health",
            Self::NotFound => "/missing",
            Self::MethodNotAllowed => "/submit",
            Self::PayloadTooLarge | Self::MalformedJson => "/body/json",
        }
    }

    const fn content_type(self) -> Option<&'static str> {
        match self {
            Self::PayloadTooLarge | Self::MalformedJson => Some("application/json"),
            Self::Success | Self::NotFound | Self::MethodNotAllowed => None,
        }
    }

    fn body(self) -> FixtureBody {
        match self {
            Self::Success | Self::NotFound | Self::MethodNotAllowed => FixtureBody::empty(),
            Self::PayloadTooLarge => FixtureBody::from_static(JSON_OVER_LIMIT),
            Self::MalformedJson => FixtureBody::from_static(JSON_MALFORMED),
        }
    }

    const fn expected(self) -> StatusCode {
        match self {
            Self::Success => StatusCode::OK,
            Self::NotFound | Self::MethodNotAllowed => StatusCode::NOT_FOUND,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::MalformedJson => StatusCode::BAD_REQUEST,
        }
    }
}

struct PreparedScenario {
    fixture: RejectionFixture,
    request: Request<FixtureBody>,
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    let mut builder = Request::builder().method(scenario.method()).uri(scenario.path());
    if let Some(content_type) = scenario.content_type() {
        builder = builder.header(http::header::CONTENT_TYPE, content_type);
    }
    let request = builder
        .body(scenario.body())
        .expect("the benchmark request metadata is valid");
    PreparedScenario {
        fixture: RejectionFixture,
        request,
    }
}

#[expect(clippy::panic, reason = "a pending in-memory dispatch future is a benchmark invariant violation")]
fn run_ready<F: Future>(future: F) -> F::Output {
    // Stack-pin to avoid allocator noise on the measured dispatch path.
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory dispatch future must complete in one poll"),
    }
}

fn run_prepared(prepared: PreparedScenario) -> StatusCode {
    run_ready(prepared.fixture.route(prepared.request, &())).status()
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        let status = run_prepared(prepare(scenario));
        assert_eq!(status, scenario.expected(), "{} produced an unexpected status", scenario.name());
    }
}
