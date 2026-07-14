// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mock event processor for testing the emit pipeline.

use std::any::type_name;
use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use observed::__private::EnrichmentEntry;
use observed::metadata::{EventDescription, InstrumentKind};
use observed::processing::{EventProcessor, EventView};
use observed::{Severity, Value};

/// A captured field-level metric emitted by an event field.
#[derive(Debug, Clone)]
pub struct CapturedFieldMetric {
    field_key: String,
    metric: observed::metadata::MetricDescription,
    value: f64,
}

impl CapturedFieldMetric {
    /// Returns the event field key that sourced this metric.
    #[must_use]
    pub fn field_key(&self) -> &str {
        &self.field_key
    }

    /// Returns the metric instrument description.
    #[must_use]
    pub fn metric(&self) -> observed::metadata::MetricDescription {
        self.metric
    }

    /// Returns the numeric value extracted from the source field.
    #[must_use]
    pub fn value(&self) -> f64 {
        self.value
    }
}

/// A captured event snapshot taken from an [`EventView`].
///
/// Stores all data needed for test assertions: name, severity, body,
/// dimensions (key-value pairs), source location, and signal metadata.
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    name: String,
    severity: Severity,
    body: Option<String>,
    source_file: Option<String>,
    source_line: Option<u32>,
    sorted_dimensions: Vec<(String, Value)>,
    field_metrics: Vec<CapturedFieldMetric>,
    description: EventDescription,
}

impl CapturedEvent {
    /// Returns the captured event name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the captured event severity.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Returns the captured event body, if present.
    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    /// Returns the source file path, if captured.
    #[must_use]
    pub fn source_file(&self) -> Option<&str> {
        self.source_file.as_deref()
    }

    /// Returns the source line number, if captured.
    #[must_use]
    pub fn source_line(&self) -> Option<u32> {
        self.source_line
    }

    /// Returns the compile-time event description (signals, metrics, fields).
    #[must_use]
    pub fn description(&self) -> &EventDescription {
        &self.description
    }

    /// Returns `true` if this event produces any metric signal.
    #[must_use]
    pub fn contains_metric(&self) -> bool {
        self.description.contains_metrics()
    }

    /// Returns the event-level metric description, if any.
    ///
    /// Field-level metric definitions are available via [`Self::field_metrics`].
    #[must_use]
    pub fn event_metric(&self) -> Option<observed::metadata::MetricDescription> {
        self.description.metric().copied()
    }

    /// Returns field-level metrics captured from metric source fields.
    #[must_use]
    pub fn field_metrics(&self) -> &[CapturedFieldMetric] {
        &self.field_metrics
    }

    /// Returns dimensions used for metric dimensions, excluding metric source fields.
    #[must_use]
    pub fn dimensions(&self) -> Vec<(String, Value)> {
        self.sorted_dimensions
            .iter()
            .filter(|(key, _)| !self.field_metrics.iter().any(|metric| metric.field_key == *key))
            .cloned()
            .collect()
    }

    fn from_event_view(event: &EventView<'_>, engine: &data_privacy::RedactionEngine) -> Self {
        let mut dimensions = Vec::new();
        let mut field_metrics = Vec::new();

        // Collect event fields (all of them, regardless of exclude flags).
        let _ = event.visit_fields(&mut |desc, get_value| {
            let value = get_value(engine);
            let key = desc
                .log()
                .map(|l| l.key().to_owned())
                .or_else(|| desc.metric().map(|m| m.key().to_owned()))
                .unwrap_or_else(|| desc.field_name().to_owned());

            if let Some(metric) = desc.metric()
                && let Some(instrument) = metric.instrument_description()
                && let Some(number) = value.to_number()
            {
                field_metrics.push(CapturedFieldMetric {
                    field_key: key.clone(),
                    metric: *instrument,
                    value: number,
                });
            }

            dimensions.push((key, value));
            ControlFlow::Continue(())
        });

        // Collect enrichments (all of them, regardless of exclude flags).
        let _ = event.visit_enrichments(&mut |desc, get_value| {
            let value = get_value(engine);
            let key = desc
                .log()
                .map(|l| l.key().to_owned())
                .or_else(|| desc.metric().map(|m| m.key().to_owned()))
                .unwrap_or_else(|| desc.field_name().to_owned());
            dimensions.push((key, value));
            ControlFlow::Continue(())
        });

        dimensions.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            name: event.name().to_owned(),
            severity: event.severity().unwrap_or(Severity::Info),
            body: event.body().map(Cow::into_owned),
            source_file: event.source_file().map(Cow::into_owned),
            source_line: event.source_line(),
            sorted_dimensions: dimensions,
            field_metrics,
            description: event.description(),
        }
    }
}

/// A mock [`EventProcessor`] that captures all emitted events for later assertion.
///
/// Clone is cheap - all clones share the same event buffer.
///
/// # Example
///
/// ```
/// use observed::{Event, Severity, Sink, SinkId, emit};
/// use observed_testing::{ExpectedEvent, MockProcessor};
///
/// static ID: SinkId = SinkId::new("mock_test");
///
/// let processor = MockProcessor::new();
/// let sink = Sink::new(
///     ID,
///     vec![std::sync::Arc::new(processor.clone())],
///     tick::SimpleClock::new_frozen(),
/// );
///
/// # #[derive(Event)]
/// # #[event(name = "test.event")]
/// # #[log(severity = info)]
/// # struct TestEvent { #[unredacted] value: i64 }
/// emit!(sink, TestEvent { value: 1 });
///
/// let expected = ExpectedEvent::new("test.event", Severity::Info)
///     .dimension("value", 1i64)
///     .log();
/// assert_eq!(processor.single_event(), expected);
/// ```
#[derive(Clone)]
pub struct MockProcessor {
    inner: Arc<MockProcessorInner>,
}

type InterestFilterFn = Box<dyn Fn(&EventDescription) -> bool + Send + Sync>;

struct MockProcessorInner {
    events: Mutex<Vec<CapturedEvent>>,
    redaction_engine: data_privacy::RedactionEngine,
    /// Optional interest filter. When `None`, all events are accepted.
    interest_filter: Option<InterestFilterFn>,
}

impl MockProcessor {
    /// Creates a new mock processor with a passthrough redaction engine.
    #[must_use]
    pub fn new() -> Self {
        Self::with_redaction_engine(passthrough_redaction_engine())
    }

    /// Creates a new mock processor with the given redaction engine.
    #[must_use]
    pub fn with_redaction_engine(redaction_engine: data_privacy::RedactionEngine) -> Self {
        Self {
            inner: Arc::new(MockProcessorInner {
                events: Mutex::new(Vec::new()),
                redaction_engine,
                interest_filter: None,
            }),
        }
    }

    /// Creates a new mock processor with a custom interest filter.
    ///
    /// Only events for which `filter` returns `true` will be captured.
    #[must_use]
    pub fn with_filter(filter: impl Fn(&EventDescription) -> bool + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(MockProcessorInner {
                events: Mutex::new(Vec::new()),
                redaction_engine: passthrough_redaction_engine(),
                interest_filter: Some(Box::new(filter)),
            }),
        }
    }

    /// Returns all captured events.
    #[must_use]
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.inner.events.lock().expect("lock poisoned").clone()
    }

    /// Returns the number of captured events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.events.lock().expect("lock poisoned").len()
    }

    /// Returns `true` if no events have been captured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the single captured event, panicking if the count is not exactly 1.
    ///
    /// This is a convenience for the common test pattern:
    /// ```ignore
    /// let events = processor.events();
    /// assert_eq!(events.len(), 1);
    /// let event = &events[0];
    /// ```
    #[must_use]
    #[track_caller]
    pub fn single_event(&self) -> CapturedEvent {
        let events = self.events();
        assert!(events.len() == 1, "expected exactly 1 captured event, got {}", events.len());
        events.into_iter().next().expect("length asserted to be 1 above")
    }
}

impl Default for MockProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MockProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.len();
        f.debug_struct(type_name::<Self>()).field("captured_events", &count).finish()
    }
}

impl EventProcessor for MockProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        match &self.inner.interest_filter {
            Some(filter) => filter(description),
            None => true,
        }
    }

    fn process(&self, event: &EventView<'_>) {
        let captured = CapturedEvent::from_event_view(event, &self.inner.redaction_engine);
        self.inner.events.lock().expect("lock poisoned").push(captured);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// Creates a passthrough [`data_privacy::RedactionEngine`] that does not
/// redact any values. Useful for tests where privacy behavior is not under test.
fn passthrough_redaction_engine() -> data_privacy::RedactionEngine {
    data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
            data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
        ))
        .build()
}

// ---------------------------------------------------------------------------
// ExpectedEvent - partial-match builder for test assertions
// ---------------------------------------------------------------------------

/// A partial-match event specification for test assertions.
///
/// Construct with [`ExpectedEvent::new`] and chain builder methods for the fields
/// you want to verify. Only specified fields are checked - unspecified fields are
/// ignored during comparison.
///
/// # Example
///
/// ```
/// use observed::Severity;
/// use observed_testing::ExpectedEvent;
///
/// let expected = ExpectedEvent::new("http.request", Severity::Info)
///     .body("Request handled")
///     .dimension("status", 200i64)
///     .log()
///     .metric();
/// ```
#[derive(Debug)]
pub struct ExpectedEvent {
    name: String,
    severity: Severity,
    body: Option<String>,
    sorted_dimensions: Vec<(String, Value)>,
    expect_log: bool,
    expect_metric: bool,
    expect_disabled: bool,
}

impl ExpectedEvent {
    /// Creates a new expected event with the given name and severity.
    ///
    /// All other fields are unchecked by default. Use builder methods to add
    /// constraints.
    #[must_use]
    pub fn new(name: impl Into<String>, severity: Severity) -> Self {
        Self {
            name: name.into(),
            severity,
            body: None,
            sorted_dimensions: Vec::new(),
            expect_log: false,
            expect_metric: false,
            expect_disabled: false,
        }
    }

    /// Expects the event to have the given body.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Expects a dimension with the given key and value.
    #[must_use]
    pub fn dimension(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.sorted_dimensions.push((key.into(), value.into()));
        self.sorted_dimensions.sort_by(|a, b| a.0.cmp(&b.0));
        self
    }

    /// Expects the event to have the LOG signal.
    #[must_use]
    pub fn log(mut self) -> Self {
        self.expect_log = true;
        self
    }

    /// Expects the event to have the METRIC signal.
    #[must_use]
    pub fn metric(mut self) -> Self {
        self.expect_metric = true;
        self
    }

    /// Expects the event to be disabled by default.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.expect_disabled = true;
        self
    }
}

impl PartialEq<ExpectedEvent> for CapturedEvent {
    fn eq(&self, other: &ExpectedEvent) -> bool {
        self.name == other.name
            && self.severity == other.severity
            && self.body.as_deref() == other.body.as_deref()
            && self.sorted_dimensions == other.sorted_dimensions
            && other.expect_log == self.description.is_log()
            && other.expect_metric == self.description.contains_metrics()
            && other.expect_disabled == self.description.is_disabled()
    }
}

impl PartialEq<CapturedEvent> for ExpectedEvent {
    fn eq(&self, other: &CapturedEvent) -> bool {
        other == self
    }
}

// ---------------------------------------------------------------------------
// ExpectedEnrichmentEntry - partial-match builder for enrichment assertions
// ---------------------------------------------------------------------------

/// A partial-match enrichment entry specification for test assertions.
///
/// Construct with [`ExpectedEnrichmentEntry::new`] and chain builder methods
/// for the attributes you want to verify. All specified attributes are checked.
///
/// # Example
///
/// ```
/// use observed_testing::ExpectedEnrichmentEntry;
///
/// let expected = ExpectedEnrichmentEntry::new("http.method", "GET")
///     .exclude_from_logs()
///     .metric_dimension();
/// ```
#[derive(Debug)]
pub struct ExpectedEnrichmentEntry {
    key: String,
    value: Value,
    exclude_from_logs: bool,
    metric_key: Option<String>,
}

impl ExpectedEnrichmentEntry {
    /// Creates a new expected enrichment entry with the given key and value.
    ///
    /// By default `exclude_from_logs` is `false` and the entry is not expected
    /// to be a metric dimension.
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            exclude_from_logs: false,
            metric_key: None,
        }
    }

    /// Expects the entry to be excluded from log records.
    #[must_use]
    pub fn exclude_from_logs(mut self) -> Self {
        self.exclude_from_logs = true;
        self
    }

    /// Expects the entry to be a metric dimension keyed by its own key.
    #[must_use]
    pub fn metric_dimension(mut self) -> Self {
        self.metric_key = Some(self.key.clone());
        self
    }

    /// Expects the entry to be a metric dimension keyed by `name`.
    #[must_use]
    pub fn metric_dimension_named(mut self, name: impl Into<String>) -> Self {
        self.metric_key = Some(name.into());
        self
    }
}

impl PartialEq<ExpectedEnrichmentEntry> for EnrichmentEntry {
    fn eq(&self, other: &ExpectedEnrichmentEntry) -> bool {
        let engine = passthrough_redaction_engine();
        self.key().as_str() == other.key
            && self.redacted_value(&engine) == other.value
            && self.is_excluded_from_logs() == other.exclude_from_logs
            && self.metric_key().map(observed::Key::as_str) == other.metric_key.as_deref()
    }
}

impl PartialEq<EnrichmentEntry> for ExpectedEnrichmentEntry {
    fn eq(&self, other: &EnrichmentEntry) -> bool {
        other == self
    }
}

// ---------------------------------------------------------------------------
// ExpectedEventDescription - partial-match builder for Event::DESCRIPTION
// ---------------------------------------------------------------------------

/// A partial-match specification for [`EventDescription`] assertions.
///
/// Construct with [`ExpectedEventDescription::new`] and chain builder methods
/// for the attributes you want to verify. All specified attributes are checked
/// against the `DESCRIPTION` constant generated by `#[derive(Event)]`.
///
/// # Example
///
/// ```
/// use observed::metadata::InstrumentKind;
/// use observed::{Event, Severity};
/// use observed_testing::ExpectedEventDescription;
///
/// # #[derive(Event)]
/// # #[event(name = "http.request")]
/// # #[log(severity = info, message = "Request")]
/// # #[metric(kind = counter, name = "http.requests")]
/// # struct HttpRequest;
/// assert_eq!(
///     HttpRequest::DESCRIPTION,
///     ExpectedEventDescription::new("http.request", Severity::Info)
///         .body("Request")
///         .log()
///         .event_metric("http.requests", InstrumentKind::Counter),
/// );
/// ```
#[derive(Debug)]
pub struct ExpectedEventDescription {
    name: String,
    severity: Severity,
    body: Option<String>,
    is_log: bool,
    is_metric: bool,
    is_disabled: bool,
    expected_event_metric: Option<(String, InstrumentKind)>,
}

impl ExpectedEventDescription {
    /// Creates a new expected event description with the given name and severity.
    ///
    /// By default signals are all `false` and no metrics are expected.
    #[must_use]
    pub fn new(name: impl Into<String>, severity: Severity) -> Self {
        Self {
            name: name.into(),
            severity,
            body: None,
            is_log: false,
            is_metric: false,
            is_disabled: false,
            expected_event_metric: None,
        }
    }

    /// Expects the event description to have the given body.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Expects the LOG signal to be set.
    #[must_use]
    pub fn log(mut self) -> Self {
        self.is_log = true;
        self
    }

    /// Expects the METRIC signal to be set.
    #[must_use]
    pub fn metric(mut self) -> Self {
        self.is_metric = true;
        self
    }

    /// Expects the DISABLED signal to be set.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.is_disabled = true;
        self
    }

    /// Expects an event-level metric description (from a fieldless struct-level `#[metric(kind = counter)]`).
    #[must_use]
    pub fn event_metric(mut self, instrument_name: impl Into<String>, kind: InstrumentKind) -> Self {
        self.expected_event_metric = Some((instrument_name.into(), kind));
        self.is_metric = true;
        self
    }
}

impl PartialEq<ExpectedEventDescription> for EventDescription {
    fn eq(&self, other: &ExpectedEventDescription) -> bool {
        let severity_matches = match self.log() {
            Some(l) => l.severity() == other.severity,
            None => !other.is_log,
        };
        let body_matches = self.log().and_then(observed::metadata::LogDescription::body) == other.body.as_deref();

        *self.name() == other.name
            && severity_matches
            && body_matches
            && self.is_log() == other.is_log
            && self.contains_metrics() == other.is_metric
            && self.is_disabled() == other.is_disabled
            && other
                .expected_event_metric
                .as_ref()
                .is_none_or(|(name, kind)| self.metric().is_some_and(|m| m.instrument_name() == name && m.kind() == *kind))
    }
}

impl PartialEq<EventDescription> for ExpectedEventDescription {
    fn eq(&self, other: &EventDescription) -> bool {
        other == self
    }
}
