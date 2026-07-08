// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug};
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use layered::{Layer, Service};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Histogram, Meter, MeterProvider};
use opentelemetry_semantic_conventions::attribute::SERVER_PORT;
use opentelemetry_semantic_conventions::metric::HTTP_CLIENT_REQUEST_DURATION;
use opentelemetry_semantic_conventions::trace::{
    ERROR_TYPE, HTTP_REQUEST_METHOD, HTTP_RESPONSE_STATUS_CODE, NETWORK_PROTOCOL_NAME, NETWORK_PROTOCOL_VERSION, SERVER_ADDRESS,
    URL_SCHEME, URL_TEMPLATE,
};
use seatbelt::{Attempt, RecoveryInfo};
use tick::SimpleClock;

use crate::error_labels::{LABEL_ABANDONED, collect_error_labels};
use crate::telemetry::{
    TelemetryAttributes, base_scope, http_method_name, network_protocol_name, network_protocol_version, server_port, url_scheme_or,
};
use crate::{HttpError, HttpRequest, HttpResponse, RequestExt, RequestHandler, Result};

/// Instrument name used when [`MetricsLayer::report_total_duration`] is
/// enabled. This is an Oxidizer-specific extension to the OpenTelemetry HTTP
/// semantic conventions, used to distinguish total request duration
/// (including all retries and hedged attempts) from per-attempt duration.
const HTTP_CLIENT_REQUEST_TOTAL_DURATION: &str = "http.client.request.total_duration";

/// Metric attribute key for the zero-based attempt index of an HTTP request.
const RESILIENCE_ATTEMPT_INDEX: &str = "resilience.attempt.index";

/// Metric attribute key indicating whether the recorded attempt is the last one
/// that will be performed.
const RESILIENCE_ATTEMPT_IS_LAST: &str = "resilience.attempt.is_last";

type CallbackType = Arc<dyn Fn(Duration, &Result<HttpResponse>, &[KeyValue]) + Send + Sync>;
type RequestEnricherFn = Arc<dyn Fn(&mut TelemetryAttributes, &HttpRequest) + Send + Sync>;
type ResponseEnricherFn = Arc<dyn Fn(&mut TelemetryAttributes, &Result<HttpResponse>) + Send + Sync>;

/// Callback invoked after metric attributes have been collected, allowing
/// callers to observe the reported attributes alongside the request result.
#[derive(Clone)]
struct OnRecordCallback(CallbackType);

impl Debug for OnRecordCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OnRecordCallback(..)")
    }
}

/// Callback that adds caller-provided attributes derived from the inbound
/// [`HttpRequest`] before the request is dispatched.
#[derive(Clone)]
struct RequestEnricher(RequestEnricherFn);

impl Debug for RequestEnricher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RequestEnricher(..)")
    }
}

/// Callback that adds caller-provided attributes derived from the request
/// outcome before metrics are recorded.
#[derive(Clone)]
struct ResponseEnricher(ResponseEnricherFn);

impl Debug for ResponseEnricher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResponseEnricher(..)")
    }
}

/// Request handler that automatically collects HTTP metrics.
///
/// Simply drop this handler in front of any existing [`RequestHandler`] to
/// automatically gather OpenTelemetry-compatible metrics for all HTTP requests.
///
/// # Metrics Collected
///
/// * **Meter name**: `fetch`
/// * **Duration**: [`http.client.request.duration`](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#metric-httpclientrequestduration) - How long requests take in seconds
///
/// When [`MetricsLayer::report_total_duration`] is enabled, the histogram is
/// recorded under the name `http.client.request.total_duration` instead,
/// distinguishing measurements that cover an entire logical request (including
/// retries and hedged attempts) from per-attempt measurements.
#[derive(Debug)]
pub struct Metrics<T> {
    inner: T,
    clock: SimpleClock,
    request_duration: Histogram<f64>,
    on_record: Option<OnRecordCallback>,
    enrich_from_request: Option<RequestEnricher>,
    enrich_from_response: Option<ResponseEnricher>,
    include_attempt: bool,
}

/// Layer that wraps a service with [`Metrics`].
///
/// Use [`MetricsLayer::on_record`] to register a callback that is
/// invoked every time a metric is recorded. This lets callers observe the
/// final set of attributes alongside the request result without needing to
/// instrument the histogram themselves.
#[derive(Debug)]
pub struct MetricsLayer {
    clock: Option<SimpleClock>,
    meter: Option<Meter>,
    report_total_duration: bool,
    on_record: Option<OnRecordCallback>,
    enrich_from_request: Option<RequestEnricher>,
    enrich_from_response: Option<ResponseEnricher>,
    include_attempt: bool,
}

impl MetricsLayer {
    /// Sets the clock used to measure request duration.
    ///
    /// When no clock is configured, a system clock ([`SimpleClock::new_system`])
    /// is used. Supplying a controlled clock (for example
    /// [`SimpleClock::new_frozen`]) is primarily useful for deterministic timing
    /// in tests.
    #[must_use]
    pub fn clock(mut self, clock: impl Into<SimpleClock>) -> Self {
        self.clock = Some(clock.into());
        self
    }

    /// Sets the [`Meter`] used to create the request-duration histogram when the
    /// layer is built.
    ///
    /// When no meter is configured, the global meter provider is used.
    #[must_use]
    pub fn meter(mut self, meter: Meter) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Sets the meter from the given [`MeterProvider`], creating the `fetch`
    /// meter used to record the request-duration histogram when the layer is
    /// built.
    ///
    /// When no meter is configured, the global meter provider is used.
    #[must_use]
    pub fn meter_provider(mut self, meter_provider: &dyn MeterProvider) -> Self {
        self.meter = Some(meter_provider.meter_with_scope(base_scope()));
        self
    }

    /// Registers a callback that is invoked each time a request metric is
    /// recorded, receiving the request duration, the result, and the
    /// collected [`KeyValue`] attributes.
    #[must_use]
    pub fn on_record(mut self, callback: impl Fn(Duration, &Result<HttpResponse>, &[KeyValue]) + Send + Sync + 'static) -> Self {
        self.on_record = Some(OnRecordCallback(Arc::new(callback)));
        self
    }

    /// Registers a closure that enriches the metric [`TelemetryAttributes`]
    /// using information from the outgoing [`HttpRequest`].
    ///
    /// The closure is invoked once per request, after the built-in request
    /// attributes have been collected and before the request is dispatched.
    /// Any attributes pushed to the provided `TelemetryAttributes` are merged
    /// into the final set of metric attributes for that request.
    #[must_use]
    pub fn enrich_from_request(mut self, enricher: impl Fn(&mut TelemetryAttributes, &HttpRequest) + Send + Sync + 'static) -> Self {
        self.enrich_from_request = Some(RequestEnricher(Arc::new(enricher)));
        self
    }

    /// Registers a closure that enriches the metric [`TelemetryAttributes`]
    /// using information from the request outcome.
    ///
    /// The closure is invoked once per request, after the built-in response
    /// or error attributes have been collected and before the histogram is
    /// recorded. Any attributes pushed to the provided `TelemetryAttributes`
    /// are merged into the final set of metric attributes for that request.
    #[must_use]
    pub fn enrich_from_response(
        mut self,
        enricher: impl Fn(&mut TelemetryAttributes, &Result<HttpResponse>) + Send + Sync + 'static,
    ) -> Self {
        self.enrich_from_response = Some(ResponseEnricher(Arc::new(enricher)));
        self
    }

    /// Controls whether resilience attempt attributes are added to the recorded
    /// metrics.
    ///
    /// When enabled, two attributes derived from the [`Attempt`] stored in the
    /// request extensions are attached to every metric record:
    ///
    /// * `resilience.attempt.index` — the zero-based attempt index.
    /// * `resilience.attempt.is_last` — whether this is the final attempt that
    ///   will be performed.
    ///
    /// When no [`Attempt`] is present in the request extensions (for example,
    /// when this layer is used outside of a retry stack, or on the first
    /// attempt), the fallback reported is index `0` with `is_last = true`,
    /// matching the semantics of a single, terminal attempt.
    ///
    /// This is disabled by default to avoid increasing metric cardinality when
    /// attempt-level granularity is not required.
    #[must_use]
    pub fn include_attempt(mut self, enabled: bool) -> Self {
        self.include_attempt = enabled;
        self
    }

    /// Toggles whether the request duration histogram is recorded under the
    /// alternative `http.client.request.total_duration` instrument name.
    ///
    /// When enabled, the layer publishes its measurements to a histogram named
    /// `http.client.request.total_duration` instead of the default
    /// [`http.client.request.duration`][HTTP_CLIENT_REQUEST_DURATION]. This is
    /// intended for layers that measure the entire request/response cycle
    /// (including retries and hedged attempts), so that downstream consumers can
    /// distinguish total and per-attempt durations without attribute
    /// disambiguation.
    #[must_use]
    pub fn report_total_duration(mut self, enabled: bool) -> Self {
        self.report_total_duration = enabled;
        self
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = Metrics<S>;

    /// Wraps the given service with a [`Metrics`] handler.
    ///
    /// The resulting handler records request metrics against the configured meter.
    fn layer(&self, inner: S) -> Self::Service {
        let meter = self
            .meter
            .clone()
            .unwrap_or_else(|| opentelemetry::global::meter_with_scope(base_scope()));
        Metrics {
            inner,
            clock: self.clock.clone().unwrap_or_else(SimpleClock::new_system),
            request_duration: build_request_duration(&meter, self.report_total_duration),
            on_record: self.on_record.clone(),
            enrich_from_request: self.enrich_from_request.clone(),
            enrich_from_response: self.enrich_from_response.clone(),
            include_attempt: self.include_attempt,
        }
    }
}

impl Metrics<()> {
    /// Creates a [`Layer`] that records request metrics.
    ///
    /// By default the global meter provider is used to create the request-duration
    /// histogram when the layer is built, and timing uses a system clock
    /// ([`SimpleClock::new_system`]). Use [`MetricsLayer::meter`] or
    /// [`MetricsLayer::meter_provider`] to record metrics against a custom meter,
    /// and [`MetricsLayer::clock`] to supply a custom clock (primarily useful for
    /// deterministic timing in tests).
    #[must_use]
    pub fn layer() -> MetricsLayer {
        MetricsLayer {
            clock: None,
            meter: None,
            report_total_duration: false,
            on_record: None,
            enrich_from_request: None,
            enrich_from_response: None,
            include_attempt: false,
        }
    }
}

/// Builds the request-duration histogram recorded by [`Metrics`].
///
/// When `report_total_duration` is `true`, the histogram is registered under
/// the `http.client.request.total_duration` instrument name; otherwise it uses
/// the standard [`http.client.request.duration`][HTTP_CLIENT_REQUEST_DURATION]
/// name.
fn build_request_duration(meter: &Meter, report_total_duration: bool) -> Histogram<f64> {
    let (name, description) = if report_total_duration {
        (
            HTTP_CLIENT_REQUEST_TOTAL_DURATION,
            "Total duration of HTTP client requests, including all retries and hedged attempts.",
        )
    } else {
        (HTTP_CLIENT_REQUEST_DURATION, "Duration of HTTP client requests.")
    };

    meter
        .f64_histogram(name)
        .with_description(description)
        .with_unit("s")
        .with_boundaries(vec![
            0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
        ])
        .build()
}

impl<T: RequestHandler> Service<HttpRequest> for Metrics<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let watch = self.clock.stopwatch();
        let mut attributes = TelemetryAttributes::default();

        fill_request_attributes(&mut attributes, &input, self.enrich_from_request.as_ref());

        if self.include_attempt {
            fill_attempt_attributes(&mut attributes, input.extensions().get::<Attempt>());
        }

        let mut guard = MetricsDropGuard {
            watch,
            attributes,
            request_duration: &self.request_duration,
            on_record: &self.on_record,
            enrich_from_response: &self.enrich_from_response,
            already_recorded: false,
        };

        self.inner.execute(input).inspect(move |r| guard.record(r))
    }
}

fn fill_response_attributes(attributes: &mut TelemetryAttributes, response: &HttpResponse) {
    attributes.push(KeyValue::new(NETWORK_PROTOCOL_NAME, network_protocol_name()));
    attributes.push(KeyValue::new(
        NETWORK_PROTOCOL_VERSION,
        network_protocol_version(response.version()),
    ));
    attributes.push(KeyValue::new(HTTP_RESPONSE_STATUS_CODE, i64::from(response.status().as_u16())));

    if let Some(values) = response.extensions().get::<TelemetryAttributes>() {
        attributes.extend(values.values().iter().cloned());
    }
}

fn fill_request_attributes(attributes: &mut TelemetryAttributes, request: &HttpRequest, enricher: Option<&RequestEnricher>) {
    attributes.push(KeyValue::new(HTTP_REQUEST_METHOD, http_method_name(request.method())));

    if let Some(val) = request.uri().authority() {
        attributes.push(KeyValue::new(SERVER_ADDRESS, val.host().to_string()));
    }

    if let Some(val) = server_port(request.uri()) {
        attributes.push(KeyValue::new(SERVER_PORT, val));
    }

    attributes.push(KeyValue::new(URL_SCHEME, url_scheme_or(request.uri().scheme())));

    if let Some(template) = request.uri_template_label() {
        attributes.push(KeyValue::new(URL_TEMPLATE, template.into_cow()));
    }

    if let Some(values) = request.extensions().get::<TelemetryAttributes>() {
        attributes.extend(values.values().iter().cloned());
    }

    if let Some(enricher) = enricher {
        (enricher.0)(attributes, request);
    }
}

fn fill_error_attributes(attributes: &mut TelemetryAttributes, error: &HttpError) {
    attributes.push(KeyValue::new(ERROR_TYPE, collect_error_labels(error).into_cow()));
}

/// Populates the resilience attempt attributes on the metric attribute set.
///
/// When `attempt` is `None`, the fallback values are an index of `0` and
/// `is_last = true`, representing a single terminal attempt.
fn fill_attempt_attributes(attributes: &mut TelemetryAttributes, attempt: Option<&Attempt>) {
    let (index, is_last) = match attempt {
        Some(attempt) => (attempt.index(), attempt.is_last()),
        None => (0, true),
    };

    attributes.push(KeyValue::new(RESILIENCE_ATTEMPT_INDEX, i64::from(index)));
    attributes.push(KeyValue::new(RESILIENCE_ATTEMPT_IS_LAST, is_last));
}

/// Drop guard that ensures metrics are recorded even when the request future
/// is cancelled.
struct MetricsDropGuard<'a> {
    watch: tick::Stopwatch,
    attributes: TelemetryAttributes,
    request_duration: &'a Histogram<f64>,
    on_record: &'a Option<OnRecordCallback>,
    enrich_from_response: &'a Option<ResponseEnricher>,
    already_recorded: bool,
}

impl MetricsDropGuard<'_> {
    /// Disarm the guard and record metrics with the actual request outcome.
    fn record(&mut self, result: &Result<HttpResponse>) {
        self.already_recorded = true;

        match result {
            Ok(response) => fill_response_attributes(&mut self.attributes, response),
            Err(err) => fill_error_attributes(&mut self.attributes, err),
        }

        if let Some(enricher) = self.enrich_from_response {
            (enricher.0)(&mut self.attributes, result);
        }

        let elapsed = self.watch.elapsed();

        if let Some(on_record) = self.on_record {
            (on_record.0)(elapsed, result, self.attributes.values());
        }

        self.request_duration.record(elapsed.as_secs_f64(), self.attributes.values());
    }
}

impl Drop for MetricsDropGuard<'_> {
    fn drop(&mut self) {
        if self.already_recorded {
            return;
        }

        self.record(&Err(HttpError::other(
            "the future has been dropped",
            RecoveryInfo::never(),
            LABEL_ABANDONED,
        )));
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Poll;

    use futures::executor::block_on;
    use http::{Request, StatusCode, Version};
    use http_extensions::{FakeHandler, HttpRequestBuilder};
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use templated_uri::{EscapedString, templated};

    use super::*;
    use crate::{HttpBodyBuilder, HttpResponseBuilder};

    fn test_layer() -> MetricsLayer {
        let provider = SdkMeterProvider::builder().build();
        Metrics::layer().meter_provider(&provider).clock(SimpleClock::new_frozen())
    }

    fn test_request() -> HttpRequest {
        Request::get("https://example.com/test")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap()
    }

    /// Collects attributes into a deterministic, snapshot-friendly representation.
    ///
    /// The result is sorted by key so the snapshot output is stable regardless of
    /// the underlying insertion order.
    #[mutants::skip]
    fn sorted_attrs(attributes: &[KeyValue]) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = attributes
            .iter()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value.as_str().into_owned()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_fill_request_attributes() {
        let request = Request::get("https://example.com/test?query=value")
            .extension(TelemetryAttributes::from_iter([KeyValue::new("extra", "extra_val")]))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        let mut attributes = TelemetryAttributes::new();

        fill_request_attributes(&mut attributes, &request, None);

        insta::assert_debug_snapshot!(sorted_attrs(attributes.values()));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_fill_request_attributes_with_template() {
        let request = HttpRequestBuilder::new_fake()
            .get(CrateUrl { crate_name: "abc".into() })
            .build()
            .unwrap();

        let mut attributes = TelemetryAttributes::new();

        fill_request_attributes(&mut attributes, &request, None);

        insta::assert_debug_snapshot!(sorted_attrs(attributes.values()));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_fill_request_attributes_with_template_label() {
        let request = HttpRequestBuilder::new_fake()
            .get(CrateUrl2 { crate_name: "abc".into() })
            .build()
            .unwrap();

        let mut attributes = TelemetryAttributes::new();

        fill_request_attributes(&mut attributes, &request, None);

        insta::assert_debug_snapshot!(sorted_attrs(attributes.values()));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_fill_response_attributes() {
        let extra = TelemetryAttributes::from_iter([KeyValue::new("extra", "extra_val")]);

        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .version(Version::HTTP_11)
            .extension(extra)
            .build()
            .unwrap();
        let mut attributes = TelemetryAttributes::new();

        fill_response_attributes(&mut attributes, &response);

        insta::assert_debug_snapshot!(sorted_attrs(attributes.values()));
    }

    #[test]
    fn many_attributes_ok() {
        let extra = (0..1000)
            .map(|v| KeyValue::new(v.to_string(), v.to_string()))
            .collect::<TelemetryAttributes>();

        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .version(Version::HTTP_11)
            .extension(extra)
            .build()
            .unwrap();
        let mut attributes = TelemetryAttributes::new();

        fill_response_attributes(&mut attributes, &response);

        assert!(attributes.values().len() > 1000);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_fill_error_attributes() {
        // Create an error
        let error = HttpError::from(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Connection refused"));

        let mut attributes = TelemetryAttributes::new();

        fill_error_attributes(&mut attributes, &error);

        insta::assert_debug_snapshot!(sorted_attrs(attributes.values()));
    }

    #[templated(template = "/api/v1/crates/{crate_name}", unredacted)]
    #[derive(Clone)]
    struct CrateUrl {
        crate_name: EscapedString,
    }

    #[templated(template = "/api/v1/crates/{crate_name}", label = "crates_api", unredacted)]
    struct CrateUrl2 {
        crate_name: EscapedString,
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_fill_request_attributes_with_url_template_label_extension() {
        use http_extensions::UriTemplateLabel;

        let mut request = Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(UriTemplateLabel::new("/api/users/{id}"));

        let mut attributes = TelemetryAttributes::new();

        fill_request_attributes(&mut attributes, &request, None);

        insta::assert_debug_snapshot!(sorted_attrs(attributes.values()));
    }

    #[cfg_attr(miri, ignore)] // insta snapshots are not supported under Miri.
    #[test]
    fn callbacks_have_compact_debug_representation() {
        let on_record = OnRecordCallback(Arc::new(|_duration, _result, _attrs| {}));
        let request_enricher = RequestEnricher(Arc::new(|_attrs, _request| {}));
        let response_enricher = ResponseEnricher(Arc::new(|_attrs, _result| {}));

        insta::assert_debug_snapshot!("on_record", on_record);
        insta::assert_debug_snapshot!("request_enricher", request_enricher);
        insta::assert_debug_snapshot!("response_enricher", response_enricher);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn on_record_is_called() {
        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);

        let handler = test_layer()
            .on_record(move |_duration, _result, _attrs| {
                flag.store(true, Ordering::Relaxed);
            })
            .layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();

        assert!(called.load(Ordering::Relaxed));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn no_callback_by_default() {
        let handler = test_layer().layer(FakeHandler::from(StatusCode::OK));

        let result = block_on(Service::execute(&handler, test_request()));
        result.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn enrich_from_request_and_response_add_attributes() {
        let recorded_attrs = Arc::new(std::sync::Mutex::new(Vec::<KeyValue>::new()));
        let attrs_clone = Arc::clone(&recorded_attrs);

        let handler = test_layer()
            .enrich_from_request(|attrs, request| {
                attrs.push(KeyValue::new("request.method", request.method().as_str().to_owned()));
                attrs.push(KeyValue::new("request.custom", "req_val"));
            })
            .enrich_from_response(|attrs, result| {
                let status = result.as_ref().map_or(-1, |r| i64::from(r.status().as_u16()));
                attrs.push(KeyValue::new("response.status", status));
                attrs.push(KeyValue::new("response.is_err", result.is_err()));
            })
            .on_record(move |_duration, _result, attrs| {
                attrs_clone.lock().unwrap().extend(attrs.iter().cloned());
            })
            .layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();

        let attrs = recorded_attrs.lock().unwrap();
        insta::assert_debug_snapshot!(sorted_attrs(&attrs));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn abandoned_future_records_abandoned_error_type() {
        let recorded_attrs = Arc::new(std::sync::Mutex::new(Vec::<KeyValue>::new()));
        let attrs_clone = Arc::clone(&recorded_attrs);

        let handler = test_layer()
            .on_record(move |_duration, _result, attrs| {
                attrs_clone.lock().unwrap().extend(attrs.iter().cloned());
            })
            .layer(FakeHandler::from_async_fn(|_req| async {
                // This future will never complete because it pends forever.
                std::future::pending::<Result<HttpResponse>>().await
            }));

        // Poll the future once so the guard is created, then drop it.
        let mut future = Box::pin(Service::execute(&handler, test_request()));

        // Should be pending because the inner handler never resolves.
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));

        // Drop the future, triggering the MetricsDropGuard.
        drop(future);

        let attrs = recorded_attrs.lock().unwrap();
        insta::assert_debug_snapshot!(sorted_attrs(&attrs));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn completed_future_does_not_record_abandoned() {
        let recorded_attrs = Arc::new(std::sync::Mutex::new(Vec::<KeyValue>::new()));
        let attrs_clone = Arc::clone(&recorded_attrs);

        let handler = test_layer()
            .on_record(move |_duration, _result, attrs| {
                attrs_clone.lock().unwrap().extend(attrs.iter().cloned());
            })
            .layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();

        let attrs = recorded_attrs.lock().unwrap();
        insta::assert_debug_snapshot!(sorted_attrs(&attrs));
    }

    /// Extracts the value recorded for a given attribute key from the collected
    /// attributes, if present.
    fn attr_value<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a opentelemetry::Value> {
        attrs.iter().find(|kv| kv.key.as_str() == key).map(|kv| &kv.value)
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn default_clock_is_used_when_unset() {
        // Not calling `.clock(..)` must fall back to a system clock and still
        // record without panicking.
        let provider = SdkMeterProvider::builder().build();
        let handler = Metrics::layer().meter_provider(&provider).layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn include_attempt_uses_extension_values() {
        let recorded_attrs = Arc::new(std::sync::Mutex::new(Vec::<KeyValue>::new()));
        let attrs_clone = Arc::clone(&recorded_attrs);

        let handler = test_layer()
            .include_attempt(true)
            .on_record(move |_duration, _result, attrs| {
                attrs_clone.lock().unwrap().extend(attrs.iter().cloned());
            })
            .layer(FakeHandler::from(StatusCode::OK));

        let mut request = test_request();
        request.extensions_mut().insert(Attempt::new(3, false));

        block_on(Service::execute(&handler, request)).unwrap();

        let attrs = recorded_attrs.lock().unwrap();
        assert_eq!(
            attr_value(&attrs, RESILIENCE_ATTEMPT_INDEX).map(opentelemetry::Value::as_str),
            Some("3".into())
        );
        assert_eq!(
            attr_value(&attrs, RESILIENCE_ATTEMPT_IS_LAST).map(opentelemetry::Value::as_str),
            Some("false".into())
        );
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn include_attempt_falls_back_when_extension_missing() {
        let recorded_attrs = Arc::new(std::sync::Mutex::new(Vec::<KeyValue>::new()));
        let attrs_clone = Arc::clone(&recorded_attrs);

        let handler = test_layer()
            .include_attempt(true)
            .on_record(move |_duration, _result, attrs| {
                attrs_clone.lock().unwrap().extend(attrs.iter().cloned());
            })
            .layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();

        let attrs = recorded_attrs.lock().unwrap();
        assert_eq!(
            attr_value(&attrs, RESILIENCE_ATTEMPT_INDEX).map(opentelemetry::Value::as_str),
            Some("0".into())
        );
        assert_eq!(
            attr_value(&attrs, RESILIENCE_ATTEMPT_IS_LAST).map(opentelemetry::Value::as_str),
            Some("true".into())
        );
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn attempt_attributes_absent_by_default() {
        let recorded_attrs = Arc::new(std::sync::Mutex::new(Vec::<KeyValue>::new()));
        let attrs_clone = Arc::clone(&recorded_attrs);

        let handler = test_layer()
            .on_record(move |_duration, _result, attrs| {
                attrs_clone.lock().unwrap().extend(attrs.iter().cloned());
            })
            .layer(FakeHandler::from(StatusCode::OK));

        let mut request = test_request();
        request.extensions_mut().insert(Attempt::new(3, false));

        block_on(Service::execute(&handler, request)).unwrap();

        let attrs = recorded_attrs.lock().unwrap();
        assert!(attr_value(&attrs, RESILIENCE_ATTEMPT_INDEX).is_none());
        assert!(attr_value(&attrs, RESILIENCE_ATTEMPT_IS_LAST).is_none());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn report_total_duration_uses_alternative_instrument_name() {
        use opentelemetry_sdk::metrics::InMemoryMetricExporter;
        use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();

        let handler = Metrics::layer()
            .meter_provider(&provider)
            .clock(SimpleClock::new_frozen())
            .report_total_duration(true)
            .layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();

        provider.force_flush().unwrap();
        let metrics = exporter.get_finished_metrics().unwrap();

        let names: Vec<&str> = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .map(opentelemetry_sdk::metrics::data::Metric::name)
            .collect();

        assert!(
            names.contains(&"http.client.request.total_duration"),
            "expected total_duration instrument, found: {names:?}"
        );
        assert!(
            !names.contains(&"http.client.request.duration"),
            "default instrument should not be present when toggle is on, found: {names:?}"
        );
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn report_total_duration_defaults_to_standard_instrument_name() {
        use opentelemetry_sdk::metrics::InMemoryMetricExporter;
        use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();

        let handler = Metrics::layer()
            .meter_provider(&provider)
            .clock(SimpleClock::new_frozen())
            .layer(FakeHandler::from(StatusCode::OK));

        block_on(Service::execute(&handler, test_request())).unwrap();

        provider.force_flush().unwrap();
        let metrics = exporter.get_finished_metrics().unwrap();

        let names: Vec<&str> = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .map(opentelemetry_sdk::metrics::data::Metric::name)
            .collect();

        assert!(names.contains(&"http.client.request.duration"), "found: {names:?}");
        assert!(!names.contains(&"http.client.request.total_duration"), "found: {names:?}");
    }
}
