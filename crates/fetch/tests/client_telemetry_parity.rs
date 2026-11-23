// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares the old request telemetry of `fetch` with the client layer of `http_telemetry`.
//!
//! The old telemetry is the pair of the `Logging` and `Metrics` handlers, which emit `tracing` events and
//! OpenTelemetry metrics. The new telemetry is `Telemetry::client`, which emits `observed` events. Each test sends
//! the same requests through both stacks and compares the values. The values must be equal, except the agreed
//! differences. Each difference is one function. It checks the difference, and then it removes the fields that it
//! explains from both sides. After that, the rest of the two sides must be equal.
//!
//! Run the tests with `--nocapture` to see the parity report. A later step deletes this file together with the old
//! handlers.

#![cfg(not(miri))] // Native tests cover this comparison, because the OpenTelemetry SDK of the old `Metrics` handler reads the OS clock.
#![allow(clippy::unwrap_used, reason = "test code")]

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Debug, Display, Write as _};
use std::future::pending;
use std::io::{self, ErrorKind};
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Waker};
use std::time::Duration;

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{DataClass, RedactionEngine, Sensitive};
use fetch::handlers::{Logging, Metrics};
use fetch::telemetry::TelemetryAttributes;
use http::{Method, Request, StatusCode, Version};
use http_extensions::{
    FakeHandler, HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, HttpResponse, HttpResponseBuilder, UriTemplateLabel,
};
use http_telemetry::Telemetry;
use layered::{Layer, Service};
use observed::enrichment::EnrichFnExt;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{Enrichment, FlushError, Severity, Sink, SinkId};
use opentelemetry::metrics::MeterProvider;
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use templated_uri::{BaseUri, EscapedString, PathAndQuery, Uri, templated};
use tick::{ClockControl, SimpleClock};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

testing_aids::init_tracing!();

/// The URI of most requests. It has an explicit port and a path without a template.
const URI: &str = "https://example.com:123/path?query=value";

/// The time that the handler takes for each request. It is exact as `f32` and as `f64` seconds.
const HANDLER_TIME: Duration = Duration::from_millis(125);

/// The bucket bounds of the old `Metrics`, in seconds.
const OLD_BOUNDS: [f64; 14] = [0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0];

/// The target of the `tracing` events of the old `Logging`.
const OLD_LOG_TARGET: &str = "fetch::handlers::logging";

/// The instrument of an attempt. Its value is also a log field under this key.
const ATTEMPT_DURATION: &str = "http.client.request.duration";

/// The instrument of a whole request. Its value is also a log field under this key.
const TOTAL_DURATION: &str = "http.client.request.total_duration";

/// The text of the panic of the handler.
const PANIC_MESSAGE: &str = "the handler panicked";

/// A method that is not one of the nine known methods.
const CUSTOM_METHOD: &str = "PURGE";

/// The identity of a client: scope attributes of the old meter, and `dimension` fields of the new layer.
const IDENTITY: [(&str, &str); 3] = [
    ("fetch.runtime", "tokio"),
    ("fetch.transport", "hyper"),
    ("http.client.name", "books"),
];

/// The key and the value of a custom dimension.
const TENANT: (&str, &str) = ("tenant", "contoso");

/// The data class of the sensitive part of [`BookPath`].
const SECRET: DataClass = DataClass::new("parity", "secret");

const PATH: &str = "url.path.redacted";
const EVENT_ROW: &str = "(event)";
const LEVEL_ROW: &str = "(level)";
const MESSAGE_ROW: &str = "(message)";
const INSTRUMENT_ROW: &str = "(instrument)";
const DESCRIPTION_ROW: &str = "(description)";
const KIND_ROW: &str = "(kind)";
const UNIT_ROW: &str = "(unit)";
const BOUNDS_ROW: &str = "(bounds)";
const SCOPE_ROW: &str = "(meter scope)";
const COUNT_ROW: &str = "(data points)";
const SECOND_PATH_ROW: &str = "url.path.redacted (second engine)";
const PANIC_ROW: &str = "(panic)";

/// The rows of a report that come before the rows of the fields.
const HEAD_ROWS: [&str; 10] = [
    EVENT_ROW,
    LEVEL_ROW,
    MESSAGE_ROW,
    INSTRUMENT_ROW,
    DESCRIPTION_ROW,
    KIND_ROW,
    UNIT_ROW,
    BOUNDS_ROW,
    SCOPE_ROW,
    COUNT_ROW,
];

/// The path of one crate. The template has no label.
#[templated(template = "/api/v1/crates/{crate_name}", unredacted)]
#[derive(Clone)]
struct CratePath {
    crate_name: EscapedString,
}

/// The path of one crate. The template has the label `crates_api`.
#[templated(template = "/api/v1/crates/{crate_name}", label = "crates_api", unredacted)]
#[derive(Clone)]
struct LabeledCratePath {
    crate_name: EscapedString,
}

/// The path of one book. The book is data of the class [`SECRET`].
#[templated(template = "/books/{book}")]
#[derive(Clone)]
struct BookPath {
    book: Sensitive<EscapedString>,
}

/// An enrichment with the metric dimension `tenant`.
#[derive(Enrichment)]
struct Tenant {
    #[dimension(metric = "tenant")]
    #[unredacted]
    tenant: &'static str,
}

fn tenant() -> Tenant {
    Tenant { tenant: TENANT.1 }
}

/// One value of a field, a metric attribute or an enrichment entry. The old log gives a `?Debug` value as its text.
#[derive(Debug, Clone, PartialEq)]
enum Val {
    Text(String),
    Int(i128),
    Float(f64),
    Bool(bool),
}

impl Val {
    fn text(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => write!(f, "{text:?}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value}"),
            Self::Bool(value) => write!(f, "{value}"),
        }
    }
}

impl From<observed::Value> for Val {
    fn from(value: observed::Value) -> Self {
        match value {
            observed::Value::Bool(value) => Self::Bool(value),
            observed::Value::I64(value) => Self::Int(i128::from(value)),
            observed::Value::U64(value) => Self::Int(i128::from(value)),
            observed::Value::F64(value) => Self::Float(value),
            observed::Value::String(text) => Self::text(text.as_str()),
            other => Self::Text(other.to_string()),
        }
    }
}

impl From<&opentelemetry::Value> for Val {
    fn from(value: &opentelemetry::Value) -> Self {
        match value {
            opentelemetry::Value::Bool(value) => Self::Bool(*value),
            opentelemetry::Value::I64(value) => Self::Int(i128::from(*value)),
            opentelemetry::Value::F64(value) => Self::Float(*value),
            opentelemetry::Value::String(text) => Self::text(text.as_str()),
            other => Self::Text(other.to_string()),
        }
    }
}

/// The log record of one request: the `tracing` event of the old `Logging`, or the log side of a new event.
#[derive(Debug, Clone, PartialEq)]
struct LogRecord {
    name: Option<String>,
    level: Option<String>,
    message: Option<String>,
    fields: BTreeMap<String, Val>,
}

/// The instrument of a data point.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Instrument {
    name: String,
    description: String,
    kind: String,
}

/// The data point of one request: the OpenTelemetry data point of the old `Metrics`, or the metric side of a new
/// event.
#[derive(Debug, Clone, PartialEq)]
struct DataPoint {
    instrument: Instrument,
    unit: Option<String>,
    bounds: Option<Vec<f64>>,
    scope: Option<String>,
    scope_attributes: BTreeMap<String, Val>,
    /// The number of measurements in the data point.
    count: Option<u64>,
    dimensions: BTreeMap<String, Val>,
    value: Option<Val>,
}

/// One enrichment entry of a new event, with its log key and its metric key.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    log: Option<String>,
    metric: Option<String>,
    value: Val,
}

/// The telemetry of one request in one stack.
#[derive(Debug, Clone, Default, PartialEq)]
struct Signals {
    log: Option<LogRecord>,
    metric: Option<DataPoint>,
    /// The enrichment entries, by field name. Only the new layer has them.
    enrichments: BTreeMap<String, Entry>,
    /// The text of the panic that came out of the stack.
    panic: Option<String>,
}

impl Signals {
    /// The keys of the fields, attributes, entries and the metric value, which are the field rows of the report.
    fn keys(&self) -> impl Iterator<Item = &str> {
        let log = self.log.iter().flat_map(|log| log.fields.keys());
        let metric = self.metric.iter().flat_map(|point| {
            point
                .dimensions
                .keys()
                .chain(point.scope_attributes.keys())
                .chain([&point.instrument.name])
        });
        log.chain(metric).chain(self.enrichments.keys()).map(String::as_str)
    }

    /// The text of `row` in the log of this stack.
    fn log_cell(&self, row: &str) -> Option<String> {
        let log = self.log.as_ref();
        match row {
            EVENT_ROW => log?.name.clone(),
            LEVEL_ROW => log?.level.clone(),
            MESSAGE_ROW => log?.message.as_ref().map(|message| format!("{message:?}")),
            PANIC_ROW => self.panic.as_ref().map(|panic| format!("{panic:?}")),
            _ => log.and_then(|log| log.fields.get(row)).map(Val::to_string).or_else(|| {
                let entry = self.enrichments.get(row)?;
                entry.log.is_some().then(|| format!("{} (enrichment)", entry.value))
            }),
        }
    }

    /// The text of `row` in the metric of this stack.
    fn metric_cell(&self, row: &str) -> Option<String> {
        let enrichment = || {
            let entry = self.enrichments.get(row)?;
            entry.metric.is_some().then(|| format!("{} (enrichment)", entry.value))
        };
        let Some(point) = &self.metric else {
            return enrichment();
        };
        match row {
            INSTRUMENT_ROW => Some(point.instrument.name.clone()),
            DESCRIPTION_ROW => Some(format!("{:?}", point.instrument.description)),
            KIND_ROW => Some(point.instrument.kind.clone()),
            UNIT_ROW => point.unit.clone(),
            BOUNDS_ROW => point.bounds.as_ref().map(|bounds| format!("{bounds:?}")),
            SCOPE_ROW => point.scope.clone(),
            COUNT_ROW => point.count.map(|count| count.to_string()),
            _ if row == point.instrument.name => point.value.as_ref().map(Val::to_string),
            _ => point
                .dimensions
                .get(row)
                .map(Val::to_string)
                .or_else(|| point.scope_attributes.get(row).map(|value| format!("{value} (meter scope)")))
                .or_else(enrichment),
        }
    }
}

/// One side of the comparison.
#[derive(Debug, Clone, Copy)]
enum Side {
    Old,
    New,
}

/// The two sides of one request. The allowed differences remove from it the fields that they explain.
struct Parity {
    label: String,
    /// The name of the instrument, which is also the log key of its value.
    instrument: &'static str,
    /// The time that the stacks measure, in seconds.
    seconds: f64,
    old: Signals,
    new: Signals,
    /// The redacted path of the old `Logging` and of the second processor, both with the second engine.
    second_paths: Option<[Option<Val>; 2]>,
    /// The differences that explain each row of the report.
    explained: BTreeMap<String, BTreeSet<&'static str>>,
    /// The name of the difference that runs now.
    current: &'static str,
}

impl Parity {
    fn signals(&self, side: Side) -> &Signals {
        match side {
            Side::Old => &self.old,
            Side::New => &self.new,
        }
    }

    fn signals_mut(&mut self, side: Side) -> &mut Signals {
        match side {
            Side::Old => &mut self.old,
            Side::New => &mut self.new,
        }
    }

    /// Records that the current difference explains `row`.
    fn explain(&mut self, row: &str) {
        self.explained.entry(row.to_owned()).or_default().insert(self.current);
    }

    fn dimension(&self, side: Side, key: &str) -> Option<Val> {
        self.signals(side).metric.as_ref()?.dimensions.get(key).cloned()
    }

    fn take_log_field(&mut self, side: Side, key: &str) -> Option<Val> {
        self.explain(key);
        self.signals_mut(side).log.as_mut()?.fields.remove(key)
    }

    fn take_dimension(&mut self, side: Side, key: &str) -> Option<Val> {
        self.explain(key);
        self.signals_mut(side).metric.as_mut()?.dimensions.remove(key)
    }

    /// Takes a part of the data point of `side`, which the report shows in `row`.
    fn take_metric_part<T>(&mut self, side: Side, row: &str, take: impl FnOnce(&mut DataPoint) -> Option<T>) -> Option<T> {
        self.explain(row);
        self.signals_mut(side).metric.as_mut().and_then(take)
    }

    fn take_value(&mut self, side: Side) -> Option<Val> {
        let row = self.instrument;
        self.take_metric_part(side, row, |point| point.value.take())
    }

    /// Takes the name and the message of the log record of `side`.
    fn take_event(&mut self, side: Side) -> [Option<String>; 2] {
        self.explain(EVENT_ROW);
        self.explain(MESSAGE_ROW);
        self.signals_mut(side)
            .log
            .as_mut()
            .map_or([None, None], |log| [log.name.take(), log.message.take()])
    }

    /// Takes the whole log record of `side`.
    fn take_log(&mut self, side: Side) -> Option<LogRecord> {
        let log = self.signals_mut(side).log.take()?;
        for row in [EVENT_ROW, LEVEL_ROW, MESSAGE_ROW]
            .into_iter()
            .chain(log.fields.keys().map(String::as_str))
        {
            self.explain(row);
        }
        Some(log)
    }

    /// Takes the whole data point of `side`.
    fn take_metric(&mut self, side: Side) -> Option<DataPoint> {
        let point = self.signals_mut(side).metric.take()?;
        let rows = [
            INSTRUMENT_ROW,
            DESCRIPTION_ROW,
            KIND_ROW,
            UNIT_ROW,
            BOUNDS_ROW,
            SCOPE_ROW,
            COUNT_ROW,
        ]
        .into_iter()
        .chain(point.dimensions.keys().map(String::as_str))
        .chain([point.instrument.name.as_str()]);
        for row in rows {
            self.explain(row);
        }
        Some(point)
    }

    /// Takes the scope attributes of the old meter.
    fn take_scope_attributes(&mut self) -> BTreeMap<String, Val> {
        let attributes = self
            .old
            .metric
            .as_mut()
            .map(|point| std::mem::take(&mut point.scope_attributes))
            .unwrap_or_default();
        for key in attributes.keys() {
            self.explain(key);
        }
        attributes
    }

    fn take_enrichment(&mut self, key: &str) -> Option<Entry> {
        self.explain(key);
        self.new.enrichments.remove(key)
    }

    fn take_panic(&mut self, side: Side) -> Option<String> {
        self.explain(PANIC_ROW);
        self.signals_mut(side).panic.take()
    }

    /// Applies `differences` in their order, and then checks that the two sides are equal.
    fn compare(&mut self, differences: &[Difference]) {
        for difference in differences {
            self.current = difference.name;
            (difference.explain)(self);
        }
        assert_eq!(
            self.second_paths, None,
            "{}: only `path_is_redacted_per_processor` reads the second processor",
            self.label
        );
        assert_eq!(
            self.old, self.new,
            "{}: the two sides differ, and no listed difference explains it",
            self.label
        );
    }

    /// The report of the request: one row for each key, with its values in the four signals and the differences
    /// that explain the row. `old`, `new` and `second_paths` are the values before the differences ran.
    fn report(&self, old: &Signals, new: &Signals, second_paths: Option<&[Option<Val>; 2]>) -> String {
        let keys: BTreeSet<&str> = old.keys().chain(new.keys()).collect();
        let mut table = vec![["key", "old log", "old metric", "new log", "new metric", "explained by"].map(String::from)];
        for row in HEAD_ROWS.into_iter().chain(keys).chain([SECOND_PATH_ROW, PANIC_ROW]) {
            let cells = if row == SECOND_PATH_ROW {
                let [old_path, new_path] = second_paths.cloned().unwrap_or_default();
                [
                    old_path.map(|path| path.to_string()),
                    None,
                    new_path.map(|path| path.to_string()),
                    None,
                ]
            } else {
                [old.log_cell(row), old.metric_cell(row), new.log_cell(row), new.metric_cell(row)]
            };
            if cells.iter().all(Option::is_none) {
                continue;
            }
            let explained = self
                .explained
                .get(row)
                .map(|names| names.iter().copied().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let [old_log, old_metric, new_log, new_metric] = cells.map(Option::unwrap_or_default);
            table.push([row.to_owned(), old_log, old_metric, new_log, new_metric, explained]);
        }
        format_table(&self.label, &table)
    }
}

/// `table` as text, with a column width that fits each cell.
fn format_table(title: &str, table: &[[String; 6]]) -> String {
    let mut widths = [0; 6];
    for row in table {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    let mut text = format!("\n{title}\n");
    for (index, row) in table.iter().enumerate() {
        for (cell, width) in row.iter().zip(widths) {
            write!(text, "| {cell:width$} ").unwrap();
        }
        text.push_str("|\n");
        if index == 0 {
            for width in widths {
                write!(text, "|{}", "-".repeat(width + 2)).unwrap();
            }
            text.push_str("|\n");
        }
    }
    text
}

/// An agreed difference between the old pair and the new layer: its name, and the function that checks it and
/// removes the fields that it explains from both sides.
#[derive(Clone, Copy)]
struct Difference {
    name: &'static str,
    explain: fn(&mut Parity),
}

/// Declares one [`Difference`] constant for each function, with the name of the function.
macro_rules! differences {
    ($($constant:ident: $function:ident),* $(,)?) => {
        $(const $constant: Difference = Difference { name: stringify!($function), explain: $function };)*
    };
}

differences! {
    EVENT_NAMES_CHANGE: event_names_change,
    METRIC_MOVES_TO_SINK: metric_moves_to_sink,
    ABANDONED_ATTEMPT_IS_LOGGED: abandoned_attempt_is_logged,
    NEVER_POLLED_ATTEMPT_IS_RECORDED: never_polled_attempt_is_recorded,
    WHOLE_REQUEST_IS_LOGGED: whole_request_is_logged,
    DURATION_IS_SECONDS: duration_is_seconds,
    PANIC_IS_ERROR: panic_is_error,
    PROTOCOL_VERSION_IS_SHORT: protocol_version_is_short,
    LOG_METHOD_IS_NORMALIZED: log_method_is_normalized,
    LOG_PORT_IS_INFERRED: log_port_is_inferred,
    LOG_SCHEME_IS_OTHER: log_scheme_is_other,
    LOG_TEMPLATE_KEY_IS_URL_TEMPLATE: log_template_key_is_url_template,
    MISSING_TEMPLATE_IS_UNKNOWN: missing_template_is_unknown,
    PROTOCOL_NAME_IS_LOGGED: protocol_name_is_logged,
    PATH_IS_REDACTED_PER_PROCESSOR: path_is_redacted_per_processor,
    IDENTITY_MOVES_TO_FIELDS: identity_moves_to_fields,
    TELEMETRY_ATTRIBUTES_ARE_IGNORED: telemetry_attributes_are_ignored,
    START_ENRICHMENT_IS_RESTORED: start_enrichment_is_restored,
}

/// The differences of a complete request whose URI has no template. The old log has the `Debug` text of the version,
/// such as `HTTP/1.1`, so each complete request also shows `protocol_version_is_short`.
const COMPLETE: [Difference; 6] = [
    MISSING_TEMPLATE_IS_UNKNOWN,
    EVENT_NAMES_CHANGE,
    METRIC_MOVES_TO_SINK,
    DURATION_IS_SECONDS,
    PROTOCOL_NAME_IS_LOGGED,
    PROTOCOL_VERSION_IS_SHORT,
];

/// The differences of a complete request whose URI has a template.
const TEMPLATED: [Difference; 6] = [
    EVENT_NAMES_CHANGE,
    METRIC_MOVES_TO_SINK,
    DURATION_IS_SECONDS,
    PROTOCOL_NAME_IS_LOGGED,
    PROTOCOL_VERSION_IS_SHORT,
    LOG_TEMPLATE_KEY_IS_URL_TEMPLATE,
];

/// The differences of a request whose handler returns an error.
const FAILED: [Difference; 5] = [
    MISSING_TEMPLATE_IS_UNKNOWN,
    EVENT_NAMES_CHANGE,
    METRIC_MOVES_TO_SINK,
    DURATION_IS_SECONDS,
    LOG_PORT_IS_INFERRED,
];

/// The differences of a request that the caller drops.
const ABANDONED: [Difference; 4] = [
    MISSING_TEMPLATE_IS_UNKNOWN,
    ABANDONED_ATTEMPT_IS_LOGGED,
    METRIC_MOVES_TO_SINK,
    DURATION_IS_SECONDS,
];

/// The differences of a request that the caller drops before its first poll.
const NEVER_POLLED: [Difference; 2] = [MISSING_TEMPLATE_IS_UNKNOWN, NEVER_POLLED_ATTEMPT_IS_RECORDED];

/// The differences of a request whose handler panics.
const PANICKED: [Difference; 4] = [
    MISSING_TEMPLATE_IS_UNKNOWN,
    PANIC_IS_ERROR,
    METRIC_MOVES_TO_SINK,
    DURATION_IS_SECONDS,
];

/// The differences of a complete request through the layers that report the whole request.
const TOTAL: [Difference; 4] = [
    MISSING_TEMPLATE_IS_UNKNOWN,
    WHOLE_REQUEST_IS_LOGGED,
    METRIC_MOVES_TO_SINK,
    DURATION_IS_SECONDS,
];

/// The differences of a complete request, and `extra`.
fn complete_and(extra: &[Difference]) -> Vec<Difference> {
    [&COMPLETE[..], extra].concat()
}

/// A name and a message, as [`Parity::take_event`] returns them.
fn event(name: &str, message: &str) -> [Option<String>; 2] {
    [Some(name.to_owned()), Some(message.to_owned())]
}

/// The old pair has the `tracing` events `http.response.complete` at DEBUG and `http.response.error` at WARN. The new
/// layer has `http.client.request.complete` at Debug and `http.client.request.error` at Warn, with new messages.
fn event_names_change(parity: &mut Parity) {
    let actual = [parity.take_event(Side::Old), parity.take_event(Side::New)];
    let complete = [
        event("http.response.complete", "HTTP response received"),
        event("http.client.request.complete", "HTTP request attempt completed"),
    ];
    let error = [
        event("http.response.error", "HTTP response failed"),
        event("http.client.request.error", "HTTP request attempt failed"),
    ];
    let expected = if actual[0] == error[0] { error } else { complete };
    assert_eq!(actual, expected, "{}", parity.label);
}

/// The old pair records an OpenTelemetry histogram on the `fetch` meter. The new event has an instrument with the
/// same name and description, and the sink records it. The test reads the meter provider of the old stack after both
/// stacks ran, so its one data point shows that the new layer records nothing there.
fn metric_moves_to_sink(parity: &mut Parity) {
    let instruments = [Side::Old, Side::New].map(|side| parity.signals(side).metric.as_ref().map(|point| point.instrument.clone()));
    let scope = parity.take_metric_part(Side::Old, SCOPE_ROW, |point| point.scope.take());
    let count = parity.take_metric_part(Side::Old, COUNT_ROW, |point| point.count.take());
    assert!(instruments[0].is_some(), "{}: the old stack records no data point", parity.label);
    assert_eq!(
        (instruments[1].clone(), scope, count),
        (instruments[0].clone(), Some("fetch".to_owned()), Some(1)),
        "{}",
        parity.label
    );
}

/// The old pair records a data point with `error.type` = `abandoned` and no log for an abandoned attempt. The new
/// layer logs `http.client.request.abandoned` at Warn, with the same data point.
fn abandoned_attempt_is_logged(parity: &mut Parity) {
    assert_eq!(
        parity.dimension(Side::Old, "error.type"),
        Some(Val::text("abandoned")),
        "{}: the old stack records no abandoned data point",
        parity.label
    );
    take_log_of_new_layer_only(parity, "http.client.request.abandoned", "WARN", "HTTP request attempt abandoned");
}

/// The old `Logging` calls the inner `Metrics` only at its first poll, so a request that the caller drops before that
/// poll gets no log and no data point. The new layer starts the request in `execute`, so it logs
/// `http.client.request.abandoned` at Warn, with a data point that has `error.type` = `abandoned` and no duration.
fn never_polled_attempt_is_recorded(parity: &mut Parity) {
    assert_eq!(
        parity.old.metric, None,
        "{}: the old stack records a request that was never polled",
        parity.label
    );
    take_log_of_new_layer_only(parity, "http.client.request.abandoned", "WARN", "HTTP request attempt abandoned");
    let point = parity
        .take_metric(Side::New)
        .map(|point| (point.instrument.name, point.dimensions.get("error.type").cloned(), point.value));
    assert_eq!(
        point,
        Some((parity.instrument.to_owned(), Some(Val::text("abandoned")), Some(Val::Float(0.0)))),
        "{}",
        parity.label
    );
}

/// The old total `Metrics` records a data point of `http.client.request.total_duration` and no log. The new total
/// layer logs `http.client.request.total.complete` at Debug, with the same data point.
fn whole_request_is_logged(parity: &mut Parity) {
    assert_eq!(
        parity.old.metric.as_ref().map(|point| point.instrument.name.as_str()),
        Some(TOTAL_DURATION),
        "{}",
        parity.label
    );
    take_log_of_new_layer_only(parity, "http.client.request.total.complete", "DEBUG", "HTTP request completed");
}

/// The old log has the duration as `f32` seconds, and the old metric as `f64` seconds with explicit buckets. The new
/// layer has `f64` seconds in both, without buckets. Each value is exactly the time that the handler took.
fn duration_is_seconds(parity: &mut Parity) {
    let seconds = Some(Val::Float(parity.seconds));
    let logged = |side| parity.signals(side).log.is_some().then_some(Val::Float(parity.seconds));
    let expected = (
        [logged(Side::Old), seconds.clone(), logged(Side::New), seconds],
        [Some("s".to_owned()), Some("s".to_owned())],
        [Some(OLD_BOUNDS.to_vec()), None],
    );
    let key = parity.instrument;
    let actual = (
        [
            parity.take_log_field(Side::Old, key),
            parity.take_value(Side::Old),
            parity.take_log_field(Side::New, key),
            parity.take_value(Side::New),
        ],
        [Side::Old, Side::New].map(|side| parity.take_metric_part(side, UNIT_ROW, |point| point.unit.take())),
        [Side::Old, Side::New].map(|side| parity.take_metric_part(side, BOUNDS_ROW, |point| point.bounds.take())),
    );
    assert_eq!(actual, expected, "{}", parity.label);
}

/// The old pair records a data point with `error.type` = `abandoned` and no log when the handler panics. The new
/// layer emits `http.client.request.error` with `error.type` = `panic`. The panic comes out of both stacks.
fn panic_is_error(parity: &mut Parity) {
    take_log_of_new_layer_only(parity, "http.client.request.error", "WARN", "HTTP request attempt failed");
    let actual = (
        [
            parity.take_dimension(Side::Old, "error.type"),
            parity.take_dimension(Side::New, "error.type"),
        ],
        [parity.take_panic(Side::Old), parity.take_panic(Side::New)],
    );
    let panic = Some(PANIC_MESSAGE.to_owned());
    assert_eq!(
        actual,
        ([Some(Val::text("abandoned")), Some(Val::text("panic"))], [panic.clone(), panic]),
        "{}",
        parity.label
    );
}

/// The old log has the `Debug` text of the version, and the old metric has `2.0` and `3.0`. The new log and metric
/// have `1.0`, `1.1`, `2` or `3`. Any other version is `_OTHER` in the old metric and in the new log and metric.
fn protocol_version_is_short(parity: &mut Parity) {
    const KEY: &str = "network.protocol.version";
    const VERSIONS: [[&str; 3]; 5] = [
        ["HTTP/1.0", "1.0", "1.0"],
        ["HTTP/1.1", "1.1", "1.1"],
        ["HTTP/2.0", "2.0", "2"],
        ["HTTP/3.0", "3.0", "3"],
        ["HTTP/0.9", "_OTHER", "_OTHER"],
    ];
    let actual = [
        parity.take_log_field(Side::Old, KEY),
        parity.take_dimension(Side::Old, KEY),
        parity.take_log_field(Side::New, KEY),
        parity.take_dimension(Side::New, KEY),
    ];
    let [old_log, old_metric, new] = VERSIONS
        .into_iter()
        .find(|[old_log, ..]| actual[0] == Some(Val::text(old_log)))
        .unwrap_or(VERSIONS[4]);
    assert_eq!(
        actual,
        [old_log, old_metric, new, new].map(|value| Some(Val::text(value))),
        "{}",
        parity.label
    );
}

/// The old log has the original method text. The new log has the known name or `_OTHER`, as the old metric, and the
/// original text in `http.request.method_original`.
fn log_method_is_normalized(parity: &mut Parity) {
    const KEY: &str = "http.request.method";
    let actual = [
        parity.take_log_field(Side::Old, KEY),
        parity.dimension(Side::Old, KEY),
        parity.take_log_field(Side::New, KEY),
        parity.take_log_field(Side::New, "http.request.method_original"),
    ];
    assert_eq!(
        actual,
        [CUSTOM_METHOD, "_OTHER", "_OTHER", CUSTOM_METHOD].map(|value| Some(Val::text(value))),
        "{}",
        parity.label
    );
}

/// The old log has the port only when the URI has one, and only on success. The new log has the port of the old
/// metric on every outcome, also the default ports 80 and 443.
fn log_port_is_inferred(parity: &mut Parity) {
    const KEY: &str = "server.port";
    let port = parity.dimension(Side::Old, KEY);
    let actual = [parity.take_log_field(Side::Old, KEY), parity.take_log_field(Side::New, KEY)];
    assert_eq!((port.is_some(), actual), (true, [None, port]), "{}", parity.label);
}

/// The old log has no `url.scheme` when the URI has no scheme. The new log has `_OTHER`, the value of the old
/// metric.
fn log_scheme_is_other(parity: &mut Parity) {
    const KEY: &str = "url.scheme";
    let actual = [
        parity.take_log_field(Side::Old, KEY),
        parity.dimension(Side::Old, KEY),
        parity.take_log_field(Side::New, KEY),
    ];
    assert_eq!(
        actual,
        [None, Some(Val::text("_OTHER")), Some(Val::text("_OTHER"))],
        "{}",
        parity.label
    );
}

/// The old log has the template under `url.path.template`. The new log has it under `url.template`, the key of the
/// metrics. All four values are equal.
fn log_template_key_is_url_template(parity: &mut Parity) {
    const KEY: &str = "url.template";
    let template = parity.dimension(Side::Old, KEY);
    let actual = [
        parity.take_log_field(Side::Old, "url.path.template"),
        parity.take_log_field(Side::New, KEY),
        parity.dimension(Side::New, KEY),
    ];
    assert_eq!(
        (template.is_some(), actual),
        (true, [template.clone(), template.clone(), template]),
        "{}",
        parity.label
    );
}

/// The old pair records no template for a request whose URI has none. The new layer logs and meters `url.template`
/// as `unknown`.
fn missing_template_is_unknown(parity: &mut Parity) {
    const KEY: &str = "url.template";
    let actual = [
        parity.take_log_field(Side::Old, "url.path.template"),
        parity.take_dimension(Side::Old, KEY),
        parity.take_log_field(Side::New, KEY),
        parity.take_dimension(Side::New, KEY),
    ];
    let unknown = Some(Val::text("unknown"));
    assert_eq!(actual, [None, None, unknown.clone(), unknown], "{}", parity.label);
}

/// `network.protocol.name` is a metric dimension of the old pair. The new layer also logs it on the complete event.
fn protocol_name_is_logged(parity: &mut Parity) {
    const KEY: &str = "network.protocol.name";
    let actual = [
        parity.take_log_field(Side::Old, KEY),
        parity.dimension(Side::Old, KEY),
        parity.take_log_field(Side::New, KEY),
    ];
    assert_eq!(actual, [None, Some(Val::text("http")), Some(Val::text("http"))], "{}", parity.label);
}

/// The old `Logging` redacts the path with the engine of its builder. The new layer gives each processor the path as
/// classified data, and each processor redacts it with its own engine. With the same engine, the texts are equal. A
/// second processor with another engine gets the text of that engine, which differs.
fn path_is_redacted_per_processor(parity: &mut Parity) {
    let first = [parity.take_log_field(Side::Old, PATH), parity.take_log_field(Side::New, PATH)];
    parity.explain(SECOND_PATH_ROW);
    let second = parity.second_paths.take();
    assert!(second.is_some(), "{}: the request has no second processor", parity.label);
    let second = second.unwrap();
    assert_eq!(
        [&first[1], &second[1]],
        [&first[0], &second[0]],
        "{}: each processor must give the text of the old `Logging` with the same engine",
        parity.label
    );
    assert_ne!(first[1], second[1], "{}: the two engines must give two texts", parity.label);
}

/// The old pair has the identity of the client as scope attributes of the meter, and no identity in the log. The new
/// layer has the same values as log fields and metric dimensions, set with `dimension`.
fn identity_moves_to_fields(parity: &mut Parity) {
    let scope = parity.take_scope_attributes();
    let fields = IDENTITY.map(|(key, _)| [parity.take_log_field(Side::New, key), parity.take_dimension(Side::New, key)]);
    let expected = IDENTITY
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Val::text(value)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        (scope, fields),
        (
            expected,
            IDENTITY.map(|(_, value)| [Some(Val::text(value)), Some(Val::text(value))])
        ),
        "{}",
        parity.label
    );
}

/// A `TelemetryAttributes` request extension adds a dimension to the old metric only. The new layer ignores it, and
/// an `observed` enrichment adds the dimension to the new events instead.
fn telemetry_attributes_are_ignored(parity: &mut Parity) {
    let (key, value) = TENANT;
    let actual = (parity.take_dimension(Side::Old, key), parity.take_enrichment(key));
    assert_eq!(actual, (Some(Val::text(value)), Some(tenant_entry())), "{}", parity.label);
}

/// `execute` runs inside an enrichment scope, and the request finishes outside it. The new event has the entry of
/// the scope. The old pair has no enrichment.
fn start_enrichment_is_restored(parity: &mut Parity) {
    assert_eq!(parity.take_enrichment(TENANT.0), Some(tenant_entry()), "{}", parity.label);
}

/// The enrichment entry of [`Tenant`].
fn tenant_entry() -> Entry {
    let (key, value) = TENANT;
    Entry {
        log: Some(key.to_owned()),
        metric: Some(key.to_owned()),
        value: Val::text(value),
    }
}

/// Checks that the old stack has no log, and that the new log has `name`, `level`, `message` and each field of the
/// new data point. Then removes the new log.
fn take_log_of_new_layer_only(parity: &mut Parity, name: &str, level: &str, message: &str) {
    assert_eq!(parity.old.log, None, "{}: the old stack logs the request", parity.label);
    let mut point = parity.new.metric.as_ref().map(|point| point.dimensions.clone()).unwrap_or_default();
    if let Some(value) = parity.new.metric.as_ref().and_then(|point| point.value.clone()) {
        point.insert(parity.instrument.to_owned(), value);
    }
    let log = parity.take_log(Side::New).map(|log| {
        let point_fields = log
            .fields
            .into_iter()
            .filter(|(key, _)| point.contains_key(key))
            .collect::<BTreeMap<_, _>>();
        (log.name, log.level, log.message, point_fields)
    });
    assert_eq!(
        log,
        Some((Some(name.to_owned()), Some(level.to_owned()), Some(message.to_owned()), point)),
        "{}: the new log must have the data point of the new metric",
        parity.label
    );
}

/// How the handler ends the request, and how the caller drives the request future. The handler moves the clock by
/// [`HANDLER_TIME`] when it is polled.
#[derive(Clone, Copy)]
enum Outcome {
    /// The handler returns a response with this status and version.
    Respond(StatusCode, Version),
    /// The handler returns the error that this function makes.
    Fail(fn() -> HttpError),
    /// The handler panics.
    Panic,
    /// The handler never returns. The caller polls the future once and then drops it.
    DropAfterPoll,
    /// The caller drops the future before its first poll.
    DropBeforePoll,
}

/// The old and the new stack of a request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stack {
    /// The old `Logging` over `Metrics`, and the new layer.
    Attempt,
    /// As `Attempt`, with the identity of the client as scope attributes of the old meter and as `dimension` fields
    /// of the new layer.
    Identity,
    /// The old `Metrics` and the new layer, both with `report_total_duration(true)`.
    Total,
}

/// How the request runs in an enrichment scope with [`Tenant`].
#[derive(Clone, Copy)]
enum TenantScope {
    /// The request runs outside the scope.
    Outside,
    /// The whole request runs inside the scope.
    Whole,
    /// `execute` runs inside the scope, and the request finishes outside it.
    Start,
}

/// One request of a scenario.
struct Case {
    label: String,
    /// Builds the request. Each stack gets its own copy.
    request: Box<dyn Fn() -> HttpRequest>,
    outcome: Outcome,
    stack: Stack,
    tenant_scope: TenantScope,
    /// The engine of a second processor of the new sink. The old `Logging` then also runs with it.
    second_engine: Option<RedactionEngine>,
    differences: Vec<Difference>,
}

impl Case {
    /// A request that gets a `200 OK` response over HTTP/1.1, through the attempt stacks.
    fn new(label: impl Into<String>, request: impl Fn() -> HttpRequest + 'static, differences: impl Into<Vec<Difference>>) -> Self {
        Self {
            label: label.into(),
            request: Box::new(request),
            outcome: Outcome::Respond(StatusCode::OK, Version::HTTP_11),
            stack: Stack::Attempt,
            tenant_scope: TenantScope::Outside,
            second_engine: None,
            differences: differences.into(),
        }
    }

    fn outcome(self, outcome: Outcome) -> Self {
        Self { outcome, ..self }
    }

    fn stack(self, stack: Stack) -> Self {
        Self { stack, ..self }
    }

    fn tenant_scope(self, tenant_scope: TenantScope) -> Self {
        Self { tenant_scope, ..self }
    }

    fn second_engine(self, engine: RedactionEngine) -> Self {
        Self {
            second_engine: Some(engine),
            ..self
        }
    }
}

/// The requests of one test. They share one clock.
struct Scenario {
    name: &'static str,
    control: ClockControl,
}

impl Scenario {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            control: ClockControl::new(),
        }
    }

    /// Sends the request of `case` through the old and the new stacks, applies the differences of the case, and
    /// checks that the rest is equal. Prints the report of the request, also when a check fails.
    fn check(&self, case: &Case) {
        let mut parity = self.run(case);
        let (old, new, second_paths) = (parity.old.clone(), parity.new.clone(), parity.second_paths.clone());
        let checked = catch_unwind(AssertUnwindSafe(|| parity.compare(&case.differences)));
        println!("{}", parity.report(&old, &new, second_paths.as_ref()));
        if let Err(payload) = checked {
            resume_unwind(payload);
        }
    }

    /// Sends the request of `case` through each stack, and collects the telemetry of both sides.
    fn run(&self, case: &Case) -> Parity {
        let handler = handler(&self.control, case.outcome);
        let engine = passthrough_engine();
        let main = SignalCapture::new(engine.clone());
        let second = case.second_engine.clone().map(SignalCapture::new);
        let mut processors: Vec<Arc<dyn EventProcessor>> = vec![Arc::new(main.clone())];
        processors.extend(second.iter().map(|second| Arc::new(second.clone()) as Arc<dyn EventProcessor>));
        let sink = Sink::builder(SinkId::new("parity"))
            .clock(SimpleClock::new_frozen())
            .processors(processors)
            .build();

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        let logs = LogCapture::default();
        let old_panic = {
            let _logs = tracing::subscriber::set_default(logs.clone());
            let metrics = Metrics::layer().clock(self.control.to_simple_clock());
            let metrics = match case.stack {
                Stack::Identity => metrics.meter(provider.meter_with_scope(identity_scope())),
                Stack::Attempt | Stack::Total => metrics.meter_provider(&provider),
            };
            match case.stack {
                Stack::Total => drive(&metrics.report_total_duration(true).layer(handler.clone()), case, &sink),
                Stack::Attempt | Stack::Identity => {
                    let logging = Logging::layer().clock(self.control.to_simple_clock()).redaction_engine(&engine);
                    drive(&logging.layer(metrics.layer(handler.clone())), case, &sink)
                }
            }
        };
        let second_old_path = case.second_engine.as_ref().and_then(|second_engine| {
            let logs = LogCapture::default();
            let _logs = tracing::subscriber::set_default(logs.clone());
            let logging = Logging::layer()
                .clock(self.control.to_simple_clock())
                .redaction_engine(second_engine);
            drive(&logging.layer(handler.clone()), case, &sink);
            logs.single().and_then(|log| log.fields.get(PATH).cloned())
        });

        let layer = Telemetry::client(sink.clone()).clock(self.control.to_simple_clock());
        let layer = match case.stack {
            Stack::Attempt => layer,
            Stack::Identity => IDENTITY.into_iter().fold(layer, |layer, (key, value)| layer.dimension(key, value)),
            Stack::Total => layer.report_total_duration(true),
        };
        let new_panic = drive(&layer.layer(handler), case, &sink);
        provider.force_flush().unwrap();

        Parity {
            label: format!("{} / {}", self.name, case.label),
            instrument: if case.stack == Stack::Total {
                TOTAL_DURATION
            } else {
                ATTEMPT_DURATION
            },
            seconds: if matches!(case.outcome, Outcome::DropBeforePoll) {
                0.0
            } else {
                HANDLER_TIME.as_secs_f64()
            },
            old: Signals {
                log: logs.single(),
                metric: old_data_point(&exporter),
                enrichments: BTreeMap::new(),
                panic: old_panic,
            },
            new: main.single().into_signals(new_panic),
            second_paths: second.map(|second| [second_old_path, second.single().log_field(PATH)]),
            explained: BTreeMap::new(),
            current: "",
        }
    }
}

/// A handler that moves the clock by [`HANDLER_TIME`] when it is polled, and then ends the request as `outcome`
/// says.
#[expect(clippy::panic, reason = "the handler of `Outcome::Panic` panics")]
fn handler(control: &ClockControl, outcome: Outcome) -> FakeHandler {
    let control = control.clone();
    match outcome {
        Outcome::Respond(status, version) => FakeHandler::from_fn(move |_| {
            control.advance(HANDLER_TIME);
            HttpResponseBuilder::new_fake().status(status).version(version).build()
        }),
        Outcome::Fail(error) => FakeHandler::from_error_fn(move |_| {
            control.advance(HANDLER_TIME);
            error()
        }),
        Outcome::Panic => FakeHandler::from_fn(move |_| {
            control.advance(HANDLER_TIME);
            panic!("{PANIC_MESSAGE}")
        }),
        Outcome::DropAfterPoll => FakeHandler::from_async_fn(move |_| {
            control.advance(HANDLER_TIME);
            pending()
        }),
        Outcome::DropBeforePoll => FakeHandler::never_completes(),
    }
}

/// Sends a new copy of the request of `case` through `stack`, and drives the future as the outcome of the case says.
/// Returns the text of the panic that came out of the stack.
fn drive(stack: &impl Service<HttpRequest, Out = http_extensions::Result<HttpResponse>>, case: &Case, sink: &Sink) -> Option<String> {
    let start = || stack.execute((case.request)());
    let send = || {
        let future = match case.tenant_scope {
            TenantScope::Start => start.enrich(sink, tenant())(),
            TenantScope::Outside | TenantScope::Whole => start(),
        };
        let mut future = pin!(future);
        let mut poll = || future.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        match case.outcome {
            Outcome::DropBeforePoll => {}
            Outcome::DropAfterPoll => assert!(poll().is_pending(), "the handler never returns"),
            Outcome::Respond(..) | Outcome::Fail(_) | Outcome::Panic => assert!(poll().is_ready(), "the handler returns at the first poll"),
        }
    };
    match (case.outcome, case.tenant_scope) {
        (Outcome::Panic, _) => catch_unwind(AssertUnwindSafe(send)).err().map(|payload| panic_text(&*payload)),
        (_, TenantScope::Whole) => {
            send.enrich(sink, tenant())();
            None
        }
        (_, TenantScope::Outside | TenantScope::Start) => {
            send();
            None
        }
    }
}

/// The text of a panic payload.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| format!("{payload:?}"))
}

/// A redaction engine that writes every classified value unchanged.
fn passthrough_engine() -> RedactionEngine {
    RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
        .build()
}

/// The scope of a `fetch` client meter, with the identity of the client, as `fetch` builds it.
fn identity_scope() -> InstrumentationScope {
    InstrumentationScope::builder("fetch")
        .with_attributes(IDENTITY.map(|(key, value)| KeyValue::new(key, value)))
        .build()
}

/// The one data point that the old stack recorded, if it recorded one.
fn old_data_point(exporter: &InMemoryMetricExporter) -> Option<DataPoint> {
    let mut points = Vec::new();
    for resource in exporter.get_finished_metrics().unwrap() {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
                    unreachable!("the old `Metrics` records only `f64` histograms");
                };
                points.extend(histogram.data_points().map(|point| DataPoint {
                    instrument: Instrument {
                        name: metric.name().to_owned(),
                        description: metric.description().to_owned(),
                        kind: "Histogram".to_owned(),
                    },
                    unit: Some(metric.unit().to_owned()),
                    bounds: Some(point.bounds().collect()),
                    scope: Some(scope.scope().name().to_owned()),
                    scope_attributes: attributes(scope.scope().attributes()),
                    count: Some(point.count()),
                    dimensions: attributes(point.attributes()),
                    value: Some(Val::Float(point.sum())),
                }));
            }
        }
    }
    assert!(points.len() <= 1, "the old stack records at most one data point: {points:?}");
    points.pop()
}

fn attributes<'a>(attributes: impl Iterator<Item = &'a KeyValue>) -> BTreeMap<String, Val> {
    attributes
        .map(|attribute| (attribute.key.as_str().to_owned(), Val::from(&attribute.value)))
        .collect()
}

/// A `tracing` subscriber for the test thread. It keeps each event of the old `Logging` as a [`LogRecord`].
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<LogRecord>>>);

impl LogCapture {
    /// The log record of the request, if the old stack logged it.
    fn single(&self) -> Option<LogRecord> {
        let mut logs = self.0.lock().unwrap();
        assert!(logs.len() <= 1, "the old stack logs at most one event for each request: {logs:?}");
        logs.pop()
    }
}

impl Subscriber for LogCapture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == OLD_LOG_TARGET
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = FieldRecorder::default();
        event.record(&mut fields);
        self.0.lock().unwrap().push(LogRecord {
            name: Some(event.metadata().name().to_owned()),
            level: Some(event.metadata().level().as_str().to_owned()),
            message: fields.message,
            fields: fields.values,
        });
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// Keeps the typed fields of one `tracing` event, and its message apart.
#[derive(Default)]
struct FieldRecorder {
    message: Option<String>,
    values: BTreeMap<String, Val>,
}

impl FieldRecorder {
    fn keep(&mut self, field: &Field, value: Val) {
        self.values.insert(field.name().to_owned(), value);
    }
}

impl Visit for FieldRecorder {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.keep(field, Val::Float(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.keep(field, Val::Int(i128::from(value)));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.keep(field, Val::Int(i128::from(value)));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.keep(field, Val::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.keep(field, Val::text(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(text);
        } else {
            self.keep(field, Val::Text(text));
        }
    }
}

/// One event of the new layer, as one processor saw it.
#[derive(Debug)]
struct NewEvent {
    name: &'static str,
    severity: Option<Severity>,
    message: Option<String>,
    logs: Vec<(&'static str, Val)>,
    dimensions: Vec<(&'static str, Val)>,
    measurements: Vec<(Instrument, String, Val)>,
    enrichments: Vec<(&'static str, Entry)>,
}

impl NewEvent {
    fn log_field(&self, key: &str) -> Option<Val> {
        self.logs
            .iter()
            .find_map(|(log_key, value)| (*log_key == key).then(|| value.clone()))
    }

    fn into_signals(self, panic: Option<String>) -> Signals {
        let mut measurements = self.measurements;
        assert!(
            measurements.len() <= 1,
            "a new event records at most one instrument: {measurements:?}"
        );
        let dimensions = unique(self.dimensions);
        Signals {
            log: self.severity.map(|severity| LogRecord {
                name: Some(self.name.to_owned()),
                level: Some(severity.as_str().to_owned()),
                message: self.message,
                fields: unique(self.logs),
            }),
            metric: measurements.pop().map(|(instrument, unit, value)| DataPoint {
                instrument,
                unit: Some(unit),
                bounds: None,
                scope: None,
                scope_attributes: BTreeMap::new(),
                count: None,
                dimensions,
                value: Some(value),
            }),
            enrichments: unique(self.enrichments),
            panic,
        }
    }
}

/// `fields` by key. Checks that each key comes once.
fn unique<T: Debug>(fields: Vec<(&'static str, T)>) -> BTreeMap<String, T> {
    let mut map = BTreeMap::new();
    for (key, value) in fields {
        let earlier = map.insert(key.to_owned(), value);
        assert!(earlier.is_none(), "the key `{key}` comes twice in one event");
    }
    map
}

/// A processor that keeps each event of the new layer. It tells log fields from metric dimensions and from the value
/// of the instrument, and it keeps the enrichment entries apart. Its engine redacts the classified values.
#[derive(Clone)]
struct SignalCapture {
    engine: RedactionEngine,
    events: Arc<Mutex<Vec<NewEvent>>>,
}

impl SignalCapture {
    fn new(engine: RedactionEngine) -> Self {
        Self {
            engine,
            events: Arc::default(),
        }
    }

    /// The event of the request.
    fn single(&self) -> NewEvent {
        let mut events = self.events.lock().unwrap();
        assert_eq!(events.len(), 1, "the new layer emits one event for each request: {events:?}");
        events.pop().unwrap()
    }
}

impl EventProcessor for SignalCapture {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let mut captured = NewEvent {
            name: event.name(),
            severity: event.severity(),
            message: event.body().map(String::from),
            logs: Vec::new(),
            dimensions: Vec::new(),
            measurements: Vec::new(),
            enrichments: Vec::new(),
        };
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            let value = Val::from(value(&self.engine));
            if let Some(log) = descriptor.log() {
                captured.logs.push((log.key(), value.clone()));
            }
            if let Some(metric) = descriptor.metric() {
                match metric.instrument_description() {
                    Some(instrument) => captured.measurements.push((
                        Instrument {
                            name: instrument.instrument_name().to_owned(),
                            description: instrument.description().to_owned(),
                            kind: instrument.kind().to_string(),
                        },
                        instrument.unit().to_owned(),
                        value,
                    )),
                    None => captured.dimensions.push((metric.key(), value)),
                }
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            let entry = Entry {
                log: descriptor.log().map(|log| log.key().to_owned()),
                metric: descriptor.metric().map(|metric| metric.key().to_owned()),
                value: Val::from(value(&self.engine)),
            };
            captured.enrichments.push((descriptor.field_name(), entry));
            ControlFlow::Continue(())
        });
        self.events.lock().unwrap().push(captured);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A request with `method` to `uri`, built as `fetch` builds it, so it has a `PathAndQuery` extension.
fn request(method: Method, uri: &str) -> HttpRequest {
    HttpRequestBuilder::new_fake().method(method).uri(uri).build().unwrap()
}

/// A `GET` request to [`URI`].
fn get() -> HttpRequest {
    request(Method::GET, URI)
}

/// A `GET` request to [`URI`] without a `PathAndQuery` extension, so both stacks read the path of the URI.
fn without_extension() -> HttpRequest {
    Request::get(URI).body(HttpBodyBuilder::new_fake().empty()).unwrap()
}

/// A `GET` request to `https://example.com:123/api/users/123` with the extension `label`.
fn with_label(label: UriTemplateLabel) -> HttpRequest {
    HttpRequestBuilder::new_fake()
        .uri("https://example.com:123/api/users/123")
        .extension(label)
        .build()
        .unwrap()
}

/// A `GET` request to the templated `path` on `https://example.com:123`.
fn templated_request(path: impl Into<PathAndQuery>) -> HttpRequest {
    let uri = Uri::default()
        .with_base(BaseUri::from_static("https://example.com:123"))
        .with_path_and_query(path);
    HttpRequestBuilder::new_fake().get(uri).build().unwrap()
}

/// A `GET` request to [`URI`] with the custom dimension [`TENANT`] in a `TelemetryAttributes` extension.
fn with_telemetry_attributes() -> HttpRequest {
    let (key, value) = TENANT;
    let attributes: TelemetryAttributes = std::iter::once(KeyValue::new(key, value)).collect();
    HttpRequestBuilder::new_fake().uri(URI).extension(attributes).build().unwrap()
}

#[test]
fn complete_response_matches_old_pair() {
    Scenario::new("complete_response_matches_old_pair").check(&Case::new("200", get, COMPLETE));
}

#[test]
fn error_statuses_match_old_pair() {
    let scenario = Scenario::new("error_statuses_match_old_pair");
    for status in [StatusCode::NOT_FOUND, StatusCode::SERVICE_UNAVAILABLE] {
        scenario.check(&Case::new(status.as_str(), get, COMPLETE).outcome(Outcome::Respond(status, Version::HTTP_11)));
    }
}

#[test]
fn handler_error_matches_old_pair() {
    Scenario::new("handler_error_matches_old_pair")
        .check(&Case::new("validation", get, FAILED).outcome(Outcome::Fail(|| HttpError::validation("the request is not valid"))));
}

#[test]
fn chained_error_matches_old_pair() {
    Scenario::new("chained_error_matches_old_pair").check(
        &Case::new("refused connection", get, FAILED)
            .outcome(Outcome::Fail(|| HttpError::from(io::Error::from(ErrorKind::ConnectionRefused)))),
    );
}

#[test]
fn dropped_request_matches_old_pair() {
    Scenario::new("dropped_request_matches_old_pair")
        .check(&Case::new("dropped after one poll", get, ABANDONED).outcome(Outcome::DropAfterPoll));
}

#[test]
fn never_polled_request_matches_old_pair() {
    Scenario::new("never_polled_request_matches_old_pair")
        .check(&Case::new("dropped before the first poll", get, NEVER_POLLED).outcome(Outcome::DropBeforePoll));
}

#[test]
fn panicking_handler_matches_old_pair() {
    Scenario::new("panicking_handler_matches_old_pair").check(&Case::new("panic", get, PANICKED).outcome(Outcome::Panic));
}

#[test]
fn protocol_versions_match_old_pair() {
    let scenario = Scenario::new("protocol_versions_match_old_pair");
    for version in [
        Version::HTTP_10,
        Version::HTTP_11,
        Version::HTTP_2,
        Version::HTTP_3,
        Version::HTTP_09,
    ] {
        scenario.check(&Case::new(format!("{version:?}"), get, COMPLETE).outcome(Outcome::Respond(StatusCode::OK, version)));
    }
}

#[test]
fn methods_match_old_pair() {
    let scenario = Scenario::new("methods_match_old_pair");
    let known = [
        Method::CONNECT,
        Method::DELETE,
        Method::GET,
        Method::HEAD,
        Method::OPTIONS,
        Method::PATCH,
        Method::POST,
        Method::PUT,
        Method::TRACE,
    ];
    for method in known {
        scenario.check(&Case::new(method.to_string(), move || request(method.clone(), URI), COMPLETE));
    }
    scenario.check(&Case::new(
        CUSTOM_METHOD,
        || request(Method::from_bytes(CUSTOM_METHOD.as_bytes()).unwrap(), URI),
        complete_and(&[LOG_METHOD_IS_NORMALIZED]),
    ));
}

#[test]
fn ports_match_old_pair() {
    let scenario = Scenario::new("ports_match_old_pair");
    let uris = [
        (URI, COMPLETE.to_vec()),
        ("https://example.com/path?query=value", complete_and(&[LOG_PORT_IS_INFERRED])),
        ("http://example.com/path?query=value", complete_and(&[LOG_PORT_IS_INFERRED])),
        ("ftp://example.com/path?query=value", COMPLETE.to_vec()),
        ("/path?query=value", complete_and(&[LOG_SCHEME_IS_OTHER])),
        ("http://[2001:db8::1]:8080/path?query=value", COMPLETE.to_vec()),
    ];
    for (uri, differences) in uris {
        scenario.check(&Case::new(uri, move || request(Method::GET, uri), differences));
    }
}

#[test]
fn schemes_match_old_pair() {
    let scenario = Scenario::new("schemes_match_old_pair");
    let uris = [
        (URI, COMPLETE.to_vec()),
        ("http://example.com:123/path?query=value", COMPLETE.to_vec()),
        ("abc://example.com:123/path?query=value", COMPLETE.to_vec()),
        ("/path?query=value", complete_and(&[LOG_SCHEME_IS_OTHER])),
    ];
    for (uri, differences) in uris {
        scenario.check(&Case::new(uri, move || request(Method::GET, uri), differences));
    }
}

#[test]
fn templates_match_old_pair() {
    let scenario = Scenario::new("templates_match_old_pair");
    scenario.check(&Case::new(
        "static label",
        || with_label(UriTemplateLabel::new_static("/api/users/{id}")),
        TEMPLATED,
    ));
    scenario.check(&Case::new(
        "owned label",
        || with_label(UriTemplateLabel::new(String::from("/api/users/{id}"))),
        TEMPLATED,
    ));
    scenario.check(&Case::new(
        "templated path",
        || {
            templated_request(CratePath {
                crate_name: EscapedString::from_static("abc"),
            })
        },
        TEMPLATED,
    ));
    scenario.check(&Case::new(
        "templated path with label",
        || {
            templated_request(LabeledCratePath {
                crate_name: EscapedString::from_static("abc"),
            })
        },
        TEMPLATED,
    ));
    scenario.check(&Case::new("path without template", get, COMPLETE));
    scenario.check(&Case::new("no extension", without_extension, COMPLETE));
}

#[test]
fn redacted_paths_match_old_pair() {
    let scenario = Scenario::new("redacted_paths_match_old_pair");
    let book = || {
        templated_request(BookPath {
            book: Sensitive::new(EscapedString::from_static("1"), SECRET),
        })
    };
    let redacted = complete_and(&[PATH_IS_REDACTED_PER_PROCESSOR]);
    scenario.check(
        &Case::new(
            "templated path with a sensitive part",
            book,
            [&TEMPLATED[..], &[PATH_IS_REDACTED_PER_PROCESSOR]].concat(),
        )
        .second_engine(RedactionEngine::default()),
    );
    scenario.check(&Case::new("plain path", get, redacted.clone()).second_engine(RedactionEngine::default()));
    scenario.check(&Case::new("no extension", without_extension, redacted).second_engine(RedactionEngine::default()));
}

#[test]
fn identity_matches_old_meter_scope() {
    Scenario::new("identity_matches_old_meter_scope")
        .check(&Case::new("identity", get, complete_and(&[IDENTITY_MOVES_TO_FIELDS])).stack(Stack::Identity));
}

#[test]
fn custom_dimensions_match_old_pair() {
    Scenario::new("custom_dimensions_match_old_pair").check(
        &Case::new(
            "attributes extension and enrichment",
            with_telemetry_attributes,
            complete_and(&[TELEMETRY_ATTRIBUTES_ARE_IGNORED]),
        )
        .tenant_scope(TenantScope::Whole),
    );
}

#[test]
fn start_enrichment_matches_old_pair() {
    Scenario::new("start_enrichment_matches_old_pair").check(
        &Case::new(
            "execute in scope, finish outside",
            get,
            complete_and(&[START_ENRICHMENT_IS_RESTORED]),
        )
        .tenant_scope(TenantScope::Start),
    );
}

#[test]
fn total_layer_matches_old_total_metrics() {
    Scenario::new("total_layer_matches_old_total_metrics").check(&Case::new("200", get, TOTAL).stack(Stack::Total));
}
