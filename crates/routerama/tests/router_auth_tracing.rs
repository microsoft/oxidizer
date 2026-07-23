// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authentication and `tracing` correlation through generated interceptors.
//!
//! This mirrors the `auth_tracing` example: a router-wide `#[before]` opens a
//! request span, authenticates, and inserts a typed principal, and a bare
//! `#[after]` records the status of every generated response. The assertions
//! read the *structured* event fields and the enclosing span rather than
//! formatted subscriber output.

#![allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers and interceptors must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::{AfterContext, Before, BeforeContext, ExtensionRef, Request, StatusCode, router};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Instrument as _, Level, Span, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt as _;

testing_aids::init_tracing!();

/// Correlation ids assigned to requests that arrive without one.
static NEXT_CORRELATION: AtomicU64 = AtomicU64::new(1);

// --- the router under test ---------------------------------------------------

/// The correlation id a transport layer may attach before routing.
#[derive(Clone, Copy, Debug)]
struct CorrelationId(u64);

/// The authenticated caller, borrowed by handlers through `ExtensionRef`.
#[derive(Clone, Debug)]
struct Principal {
    id: u64,
    name: &'static str,
}

/// The request span, carried through the typed request extensions.
#[derive(Clone, Debug)]
struct RequestSpan(Span);

#[derive(Clone, Copy, Debug)]
struct Orders;

#[router]
impl Orders {
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/orders/{id}")]
    async fn order(&self, id: &str, principal: ExtensionRef<'_, Principal>, trace: ExtensionRef<'_, RequestSpan>) -> String {
        let span = &trace.get().0;
        let total = load_total(id).instrument(span.clone()).await;
        span.in_scope(|| {
            tracing::event!(name: "order.served", Level::INFO, order.id = id, principal.id = principal.id);
        });
        format!("order {id} for {} totals {total}", principal.name)
    }

    /// A typed capture, so a malformed path becomes an extraction rejection
    /// that only the bare `#[after]` observes.
    #[route(GET, "/reports/{year}")]
    async fn report(&self, year: u32) -> String {
        format!("report {year}")
    }

    #[before]
    async fn authenticate(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let correlation = ctx
            .get_extension::<CorrelationId>()
            .copied()
            .unwrap_or_else(|| CorrelationId(NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed)));

        let span = tracing::info_span!(
            "http.request",
            correlation = correlation.0,
            http.method = %ctx.method(),
            http.path = ctx.uri().path(),
        );

        let public = ctx.uri().path() == "/health";
        ctx.insert_extension(correlation);
        ctx.insert_extension(RequestSpan(span.clone()));

        if public {
            return Before::Next;
        }

        let Some(principal) = ctx
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .and_then(account)
        else {
            span.in_scope(|| {
                tracing::event!(name: "auth.rejected", Level::WARN, auth.reason = "missing or unknown bearer token");
            });
            return Before::Respond(StatusCode::UNAUTHORIZED);
        };

        span.in_scope(|| {
            tracing::event!(name: "auth.accepted", Level::INFO, principal.id = principal.id, principal.name = principal.name);
        });
        ctx.insert_extension(principal);
        Before::Next
    }

    #[after]
    async fn record_outcome(&self, ctx: &mut AfterContext<'_>) {
        let status = ctx.status();

        if let Some(trace) = ctx.request().extensions.get::<RequestSpan>() {
            trace.0.in_scope(|| {
                tracing::event!(name: "http.response", Level::INFO, http.status = status.as_u16());
            });
        }

        if let Some(correlation) = ctx.request().extensions.get::<CorrelationId>() {
            let value = correlation
                .0
                .to_string()
                .parse()
                .expect("a decimal integer is a valid header value");
            _ = ctx.headers_mut().insert("x-request-id", value);
        }
    }
}

/// Stands in for handler I/O; its event proves the span survives an await.
async fn load_total(id: &str) -> usize {
    tokio::task::yield_now().await;
    tracing::event!(name: "order.loaded", Level::DEBUG, order.id = id);
    id.len() * 10
}

fn account(token: &str) -> Option<Principal> {
    match token {
        "token-ada" => Some(Principal { id: 1, name: "ada" }),
        "token-grace" => Some(Principal { id: 2, name: "grace" }),
        _ => None,
    }
}

// --- structured event capture -------------------------------------------------

/// One recorded `tracing` event plus the span that was current when it fired.
#[derive(Clone, Debug)]
struct Captured {
    name: &'static str,
    level: Level,
    fields: BTreeMap<String, String>,
    span: Option<CapturedSpan>,
}

/// The name and fields of the span an event was emitted inside.
#[derive(Clone, Debug)]
struct CapturedSpan {
    name: &'static str,
    fields: BTreeMap<String, String>,
}

impl Captured {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Returns the correlation id of the span this event was emitted inside.
    fn correlation(&self) -> Option<&str> {
        let span = self.span.as_ref()?;
        assert_eq!(span.name, "http.request", "events must be emitted inside the request span");
        span.fields.get("correlation").map(String::as_str)
    }
}

/// A `tracing` layer that records events and their enclosing span's fields.
#[derive(Debug)]
struct Recorder {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S> Layer<S> for Recorder
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        attrs.record(&mut FieldCollector(&mut fields));
        let span = ctx.span(id).expect("the span was just created");
        _ = span.extensions_mut().replace(SpanFields(fields));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));

        let span = ctx.event_span(event).map(|span| CapturedSpan {
            name: span.name(),
            fields: span
                .extensions()
                .get::<SpanFields>()
                .map(|recorded| recorded.0.clone())
                .unwrap_or_default(),
        });

        self.events.lock().expect("the recorder mutex is not poisoned").push(Captured {
            name: event.metadata().name(),
            level: *event.metadata().level(),
            fields,
            span,
        });
    }
}

/// The recorded fields of one span, stored in that span's extensions.
#[derive(Debug)]
struct SpanFields(BTreeMap<String, String>);

/// Collects `tracing` field values as strings.
struct FieldCollector<'fields>(&'fields mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        _ = self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        _ = self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        _ = self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn core::fmt::Debug) {
        _ = self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

/// Scopes a recorder to the current thread and returns the captured events.
struct Session {
    events: Arc<Mutex<Vec<Captured>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl Session {
    fn start() -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing_subscriber::registry()
            .with(Recorder {
                events: Arc::clone(&events),
            })
            .set_default();
        Self { events, _guard: guard }
    }

    fn captured(&self) -> Vec<Captured> {
        self.events.lock().expect("the recorder mutex is not poisoned").clone()
    }
}

fn find<'events>(events: &'events [Captured], name: &str) -> Option<&'events Captured> {
    events.iter().find(|event| event.name == name)
}

#[expect(clippy::panic, reason = "a missing event must fail the test with a name in the message")]
fn expect_event<'events>(events: &'events [Captured], name: &str) -> &'events Captured {
    find(events, name).unwrap_or_else(|| panic!("no `{name}` event was recorded"))
}

async fn body_bytes<B>(response: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: core::fmt::Debug,
{
    response.into_body().collect().await.expect("the body succeeds").to_bytes()
}

fn request(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::get(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).expect("the request is valid")
}

// --- tests --------------------------------------------------------------------

#[tokio::test]
async fn authenticated_request_is_served_and_traced_within_one_span() {
    let session = Session::start();

    let response = Orders.route(request("/orders/9f3", Some("token-ada")), &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    let correlation_header = response.headers()["x-request-id"].to_str().expect("ASCII header").to_owned();
    assert_eq!(body_bytes(response).await, b"order 9f3 for ada totals 30"[..]);

    let events = session.captured();
    let accepted = expect_event(&events, "auth.accepted");
    assert_eq!(accepted.level, Level::INFO);
    assert_eq!(accepted.field("principal.name"), Some("ada"));
    assert_eq!(accepted.field("principal.id"), Some("1"));

    // Emitted from a future that was `instrument`ed by the handler, which
    // proves the span stays current across an await without an entered guard.
    let loaded = expect_event(&events, "order.loaded");
    assert_eq!(loaded.level, Level::DEBUG);
    assert_eq!(loaded.field("order.id"), Some("9f3"));

    let served = expect_event(&events, "order.served");
    assert_eq!(served.field("order.id"), Some("9f3"));

    let recorded = expect_event(&events, "http.response");
    assert_eq!(recorded.field("http.status"), Some("200"));

    // Every event correlates with the same request span, and the span's
    // correlation id is the one stamped onto the response.
    let correlation = accepted.correlation().expect("the auth event has a span");
    assert_eq!(correlation, correlation_header);
    for event in &events {
        assert_eq!(event.correlation(), Some(correlation), "`{}` lost span correlation", event.name);
    }

    let span = accepted.span.as_ref().expect("the auth event has a span");
    assert_eq!(span.fields.get("http.method").map(String::as_str), Some("GET"));
    assert_eq!(span.fields.get("http.path").map(String::as_str), Some("/orders/9f3"));
}

#[tokio::test]
async fn anonymous_request_short_circuits_and_after_records_the_rejection() {
    let session = Session::start();

    let response = Orders.route(request("/orders/9f3", None), &()).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("x-request-id"), "the short-circuit is correlated");

    let events = session.captured();
    let rejected = expect_event(&events, "auth.rejected");
    assert_eq!(rejected.level, Level::WARN);
    assert_eq!(rejected.field("auth.reason"), Some("missing or unknown bearer token"));

    // The handler never ran.
    assert!(find(&events, "order.served").is_none());
    assert!(find(&events, "order.loaded").is_none());

    // The bare `#[after]` still observed the short-circuit response.
    let recorded = expect_event(&events, "http.response");
    assert_eq!(recorded.field("http.status"), Some("401"));
    assert_eq!(recorded.correlation(), rejected.correlation());
}

#[tokio::test]
async fn unknown_credential_is_rejected() {
    let session = Session::start();

    let response = Orders.route(request("/orders/9f3", Some("token-nobody")), &()).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let events = session.captured();
    _ = expect_event(&events, "auth.rejected");
    assert_eq!(expect_event(&events, "http.response").field("http.status"), Some("401"));
}

#[tokio::test]
async fn routing_failure_is_recorded_by_the_bare_after() {
    let session = Session::start();

    let response = Orders.route(request("/absent", Some("token-grace")), &()).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.headers().contains_key("x-request-id"));

    let events = session.captured();
    let recorded = expect_event(&events, "http.response");
    assert_eq!(recorded.field("http.status"), Some("404"));
    assert!(recorded.correlation().is_some(), "a routing failure is still correlated");
}

#[tokio::test]
async fn capture_rejection_is_recorded_by_the_bare_after() {
    let session = Session::start();

    let response = Orders.route(request("/reports/not-a-year", Some("token-grace")), &()).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let events = session.captured();
    let recorded = expect_event(&events, "http.response");
    assert_eq!(recorded.field("http.status"), Some("400"));
    assert!(recorded.correlation().is_some(), "an extraction rejection is still correlated");
}

#[tokio::test]
async fn public_route_skips_authentication_but_is_still_traced() {
    let session = Session::start();

    let response = Orders.route(request("/health", None), &()).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let events = session.captured();
    assert!(find(&events, "auth.accepted").is_none());
    assert!(find(&events, "auth.rejected").is_none());
    assert_eq!(expect_event(&events, "http.response").field("http.status"), Some("204"));
}

#[tokio::test]
async fn each_request_gets_its_own_correlation_id() {
    let session = Session::start();

    _ = Orders.route(request("/health", None), &()).await;
    _ = Orders.route(request("/health", None), &()).await;

    let events = session.captured();
    let correlations: Vec<_> = events
        .iter()
        .filter(|event| event.name == "http.response")
        .map(|event| event.correlation().expect("a response event has a span").to_owned())
        .collect();
    assert_eq!(correlations.len(), 2);
    assert_ne!(correlations[0], correlations[1]);
}
