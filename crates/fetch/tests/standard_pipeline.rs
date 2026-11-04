// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the standard pipeline exercising the circuit breaker,
//! retries, hedging, and per-origin isolation.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::pipeline::RecoveryMode;
use fetch::{HttpClient, HttpError, HttpResponseBuilder};
use http::StatusCode;
use http_extensions::routing::Router;
use ohno::ErrorExt;
use seatbelt::retry::Attempt;
use seatbelt::{Recovery, RecoveryKind};
use templated_uri::BaseUri;
use tick::ClockControl;

const FAILING_HOST: &str = "https://failing.example.com";
const HEALTHY_HOST: &str = "https://healthy.example.com";

// Helpers.

/// Counts fake-handler calls.
#[derive(Clone)]
struct Calls(Arc<AtomicUsize>);

impl Calls {
    /// Creates a new counter starting at zero.
    fn new() -> Self {
        Self(Arc::new(AtomicUsize::new(0)))
    }

    /// Atomically increments the counter by one.
    fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the current value of the counter.
    fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

/// Sends `count` GET requests to `uri` and discards the results.
async fn send_requests(client: &HttpClient, uri: &str, count: usize) {
    for _ in 0..count {
        let _ = client.get(uri).fetch().await;
    }
}

/// Creates a standard-pipeline client that selects responses by host.
///
/// `failing.example.com` → 500 Internal Server Error
/// everything else       → 200 OK
fn create_per_host_client(calls: Calls) -> HttpClient {
    let handler = FakeHandler::from_fn(move |req| {
        calls.increment();

        let status = if req.uri().host() == Some("failing.example.com") {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        };

        HttpResponseBuilder::new_fake().status(status).build()
    });

    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    HttpClient::builder_fake(handler, FakeDeps { clock })
        .standard_pipeline(|pipeline, _| {
            pipeline
                // Large timeouts keep auto-advance from firing before the test completes.
                .total_timeout(|t| t.timeout(Duration::MAX))
                .attempt_timeout(|t| t.timeout(Duration::MAX))
                .retry(|retry| retry.max_retry_attempts(3).base_delay(Duration::from_millis(1)))
                .breaker(|breaker| {
                    breaker
                        .min_throughput(5)
                        .failure_threshold(0.5)
                        .break_duration(Duration::from_mins(1))
                })
        })
        .build()
}

/// Creates a client where every request returns the given status code.
fn create_uniform_client(status: StatusCode, calls: Calls) -> HttpClient {
    let handler = FakeHandler::from_fn(move |_req| {
        calls.increment();
        HttpResponseBuilder::new_fake().status(status).build()
    });

    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    HttpClient::builder_fake(handler, FakeDeps { clock })
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|t| t.timeout(Duration::MAX))
                .attempt_timeout(|t| t.timeout(Duration::MAX))
                .retry(|retry| retry.max_retry_attempts(3).base_delay(Duration::from_millis(1)))
                .breaker(|breaker| {
                    breaker
                        .min_throughput(5)
                        .failure_threshold(0.5)
                        .break_duration(Duration::from_mins(1))
                })
        })
        .build()
}

// Breaker opens after server errors.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_trips_after_repeated_server_errors() {
    let calls = Calls::new();
    let client = create_uniform_client(StatusCode::INTERNAL_SERVER_ERROR, calls.clone());

    // Each logical request makes one initial attempt and three retries. The configured throughput
    // and failure threshold open the breaker after enough failures.
    send_requests(&client, "https://example.com", 10).await;

    let calls_before = calls.get();

    // The open breaker rejects the request before the handler.
    let error = client.get("https://example.com").fetch().await.unwrap_err();

    assert!(
        error.message().contains("circuit breaker"),
        "expected 'circuit breaker' in message, got: {}",
        error.message()
    );
    assert_eq!(error.recovery().kind(), RecoveryKind::Unavailable);

    // The rejection does not call the handler.
    assert_eq!(calls.get(), calls_before);
}

// Retries run before the breaker opens.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn retries_happen_before_breaker_trips() {
    let calls = Calls::new();
    let client = create_uniform_client(StatusCode::INTERNAL_SERVER_ERROR, calls.clone());

    // One logical request makes one initial attempt and three retries.
    let response = client.get("https://example.com").fetch().await.unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let attempt = response.extensions().get::<Attempt>().copied().unwrap();
    assert_eq!(attempt, Attempt::new(3, true));
    assert_eq!(calls.get(), 4);
}

// Success does not open the breaker.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn success_does_not_trip_breaker() {
    let calls = Calls::new();
    let client = create_uniform_client(StatusCode::OK, calls.clone());

    send_requests(&client, "https://example.com", 20).await;

    // Each request calls the handler once without retries or rejections.
    assert_eq!(calls.get(), 20);
}

// Client errors do not open the breaker.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_errors_do_not_trip_breaker() {
    let calls = Calls::new();
    let client = create_uniform_client(StatusCode::BAD_REQUEST, calls.clone());

    send_requests(&client, "https://example.com", 20).await;

    // Client errors are not retried and do not trip the breaker.
    assert_eq!(calls.get(), 20);
}

// Breaker state is isolated by origin.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_is_isolated_per_origin() {
    let calls = Calls::new();
    let client = create_per_host_client(calls.clone());

    // Open the breaker for the failing host.
    send_requests(&client, FAILING_HOST, 20).await;

    // The failing host now rejects requests.
    let error = client.get(FAILING_HOST).fetch().await.unwrap_err();
    assert!(
        error.message().contains("circuit breaker"),
        "expected breaker open for failing host, got: {}",
        error.message()
    );

    // The healthy host still succeeds.
    let response = client.get(HEALTHY_HOST).fetch().await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "healthy host must not be affected by the failing host's breaker"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn healthy_host_stays_healthy_while_failing_host_is_broken() {
    let calls = Calls::new();
    let client = create_per_host_client(calls.clone());

    // Send requests to both hosts. Breaker state is per origin.
    send_requests(&client, HEALTHY_HOST, 20).await;
    send_requests(&client, FAILING_HOST, 20).await;

    // The failing host rejects the request.
    let error = client.get(FAILING_HOST).fetch().await.unwrap_err();
    assert!(error.message().contains("circuit breaker"));

    // The healthy host accepts the request.
    let response = client.get(HEALTHY_HOST).fetch().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// A rejected error contains the original request.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn breaker_rejection_attaches_request() {
    let calls = Calls::new();
    let client = create_uniform_client(StatusCode::INTERNAL_SERVER_ERROR, calls.clone());

    send_requests(&client, "https://example.com", 20).await;

    let mut error = client.get("https://example.com").fetch().await.unwrap_err();

    assert!(error.message().contains("circuit breaker"));
    assert!(error.take_request().is_some(), "rejected error must contain the request");
}

// Retry observes a breaker that opens between request sequences.

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn retry_receives_breaker_rejection() {
    // Open the breaker with an earlier batch before the next request arrives.
    let calls = Calls::new();
    let client = create_uniform_client(StatusCode::INTERNAL_SERVER_ERROR, calls.clone());

    // Open the breaker.
    send_requests(&client, "https://example.com", 20).await;

    let calls_before = calls.get();

    // The retry layer receives the rejection before the handler.
    let error = client.get("https://example.com").fetch().await.unwrap_err();

    assert!(error.message().contains("circuit breaker"));
    assert_eq!(calls.get(), calls_before, "an open breaker must not call the handler");
}

// Hedging returns a later successful attempt.

const HEDGING_DELAY: Duration = Duration::from_millis(100);

/// Creates a hedging client whose handler returns status codes from the
/// given iterator in order.
fn create_hedging_client(calls: Calls, responses: Vec<StatusCode>) -> HttpClient {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let responses = Arc::new(std::sync::Mutex::new(responses.into_iter()));

    let handler = FakeHandler::from_fn(move |_req| {
        calls.increment();
        let status = responses.lock().unwrap().next().unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
        HttpResponseBuilder::new_fake().status(status).build()
    });

    HttpClient::builder_fake(handler, FakeDeps { clock })
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|t| t.timeout(Duration::MAX))
                .attempt_timeout(|t| t.timeout(Duration::MAX))
                .recovery_mode(RecoveryMode::Hedging)
                .hedging(|hedging| hedging.max_hedged_attempts(1).hedging_delay(HEDGING_DELAY))
        })
        .build()
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn hedging_returns_success_response() {
    let calls = Calls::new();
    // The first attempt returns 500. The hedge returns 200 OK.
    let client = create_hedging_client(calls.clone(), vec![StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]);

    let response = client.get("https://example.com").fetch().await.unwrap();

    // The result comes from the successful hedge.
    assert_eq!(response.status(), StatusCode::OK);

    // Dispatch the original request and one hedge.
    assert_eq!(calls.get(), 2);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn hedging_returns_first_success_immediately() {
    let calls = Calls::new();
    // The first attempt succeeds, so no hedge starts.
    let client = create_hedging_client(calls.clone(), vec![StatusCode::OK, StatusCode::OK]);

    let response = client.get("https://example.com").fetch().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // Dispatch only the initial attempt.
    assert_eq!(calls.get(), 1, "a successful first attempt must not start a hedge");
}

// Fallback routing.

const PRIMARY_HOST: &str = "primary.example.com";
const SECONDARY_HOST: &str = "secondary.example.com";

/// Counts fake-handler calls for each host.
#[derive(Clone, Default)]
struct HostCalls {
    primary: Arc<AtomicUsize>,
    secondary: Arc<AtomicUsize>,
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn fallback_router_recovers_when_primary_is_unavailable() {
    let calls = HostCalls::default();
    let handler_calls = calls.clone();

    // The primary returns unavailable. Because the router has an alternative, the retry layer
    // routes the next attempt to the fallback.
    let handler = FakeHandler::from_fn(move |req| match req.uri().host() {
        Some(PRIMARY_HOST) => {
            handler_calls.primary.fetch_add(1, Ordering::Relaxed);
            Err(HttpError::unavailable("primary is down").with_request(req))
        }
        Some(SECONDARY_HOST) => {
            handler_calls.secondary.fetch_add(1, Ordering::Relaxed);
            HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
        }
        other => panic!("unexpected host: {other:?}"),
    });

    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(handler, FakeDeps { clock })
        .router(Router::fallback(
            BaseUri::from_static("https://primary.example.com/"),
            BaseUri::from_static("https://secondary.example.com/"),
        ))
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|t| t.timeout(Duration::MAX))
                .attempt_timeout(|t| t.timeout(Duration::MAX))
                .retry(|retry| retry.max_retry_attempts(3).base_delay(Duration::from_millis(1)))
        })
        .build();

    let response = client.get("/foo").fetch().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // The initial attempt uses the primary. One retry uses the fallback and succeeds.
    let attempt = response.extensions().get::<Attempt>().copied().unwrap();
    assert_eq!(attempt, Attempt::new(1, false));
    assert_eq!(
        calls.primary.load(Ordering::Relaxed),
        1,
        "the primary endpoint must receive exactly one attempt"
    );
    assert_eq!(
        calls.secondary.load(Ordering::Relaxed),
        1,
        "the fallback endpoint must serve the request exactly once"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn hedging_does_not_clone_unsafe_methods() {
    let calls = Calls::new();
    let client = create_hedging_client(calls.clone(), vec![StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]);

    // The default clone strategy does not clone POST, so no hedge starts.
    let response = client.post("https://example.com").fetch().await.unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(calls.get(), 1, "an unsafe method must not start a hedge");
}

testing_aids::init_tracing!();
