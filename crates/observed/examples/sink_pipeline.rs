// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates the Sink processor model with per-field routing.
//!
//! Events flow through an [`Sink`] that dispatches to one or more
//! [`EventProcessor`]s. Each processor has its own redaction engine and
//! receives pre-redacted events.
//!
//! Field routing (which fields go to logs vs metric dimensions) is declared
//! inside `#[derive(Event)]` attributes. Fields are log attributes by default;
//! metric dimensions are opt-in via `#[dimension(metric = "...")]`, and
//! `#[dimension(log = exclude)]`
//! opts a field out of logs.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example sink_pipeline
//! ```

use std::sync::Arc;

use data_privacy::DataClass;
use observed::{Event, Sink, emit};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

const DC: DataClass = DataClass::new("example", "public");

// ---------------------------------------------------------------------------
// Event types with declarative routing via #[derive(Event)] attributes
// ---------------------------------------------------------------------------

/// An HTTP request event with **declarative** log and metric routing.
///
/// Every field is a log attribute by default. Use `#[dimension(metric = "...")]`
/// to opt a field into metric dimensions, or `#[dimension(log = exclude)]` to
/// drop it from logs.
#[derive(Event)]
#[event(name = "http.server.request")]
#[log(severity = info, message = "HTTP request completed")]
#[metric(kind = histogram, field = duration_ms, name = "http.server.request.duration")]
struct HttpServerRequest {
    /// The request duration - recorded as a histogram metric.
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    duration_ms: f64,

    /// HTTP status code - included in logs and, via `#[dimension(metric = "...")]`, in metrics.
    #[dimension(metric = "status")]
    #[data_class(DC)]
    status: i64,

    /// Retry count - logged only (no `#[dimension]`, so not a metric dimension).
    #[data_class(DC)]
    retries: i64,
}

/// A database query event.
#[derive(Event)]
#[event(name = "db.query")]
#[log(severity = info)]
struct DbQuery {
    #[data_class(DC)]
    query_ms: f64,
    #[data_class(DC)]
    table_id: i64,
    #[data_class(DC)]
    rows_returned: i64,
}

/// A simple processor that forwards events to an `OTel` logger provider.
struct SimpleLogProcessor {
    logger: opentelemetry_sdk::logs::SdkLogger,
    redaction_engine: data_privacy::RedactionEngine,
}

impl observed::processing::EventProcessor for SimpleLogProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        use std::ops::ControlFlow;

        use opentelemetry::logs::{AnyValue, LogRecord, Logger};

        let mut record = self.logger.create_log_record();
        record.set_event_name(event.name());
        if let Some(severity) = event.severity() {
            record.set_severity_number(opentelemetry::logs::Severity::from(severity));
            record.set_severity_text(severity.as_str());
        }
        record.set_timestamp(std::time::SystemTime::now());
        if let Some(body) = event.body() {
            record.set_body(AnyValue::String(body.into_owned().into()));
        }

        let engine = &self.redaction_engine;
        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });
        if let Some(file) = event.source_file() {
            record.add_attribute(
                opentelemetry::Key::from_static_str("code.file.path"),
                AnyValue::String(file.into_owned().into()),
            );
        }
        if let Some(line) = event.source_line() {
            record.add_attribute(
                opentelemetry::Key::from_static_str("code.line.number"),
                AnyValue::Int(i64::from(line)),
            );
        }
        self.logger.emit(record);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

fn main() {
    // --- Set up `OTel` provider ---

    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder().with_simple_exporter(log_exporter.clone()).build();

    // --- Assemble and install the Sink ---

    let sink = Sink::new(
        "sink_pipeline",
        vec![Arc::new(SimpleLogProcessor {
            logger: logger_provider.logger("app"),
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        tick::SimpleClock::new_system(),
    );

    // --- Emit events ---

    println!("=== HttpServerRequest events ===");
    emit!(
        sink,
        HttpServerRequest {
            duration_ms: 12.5,
            status: 200,
            retries: 0,
        }
    );
    emit!(
        sink,
        HttpServerRequest {
            duration_ms: 150.0,
            status: 503,
            retries: 3,
        }
    );

    println!("=== DbQuery events ===");
    emit!(
        sink,
        DbQuery {
            query_ms: 4.2,
            table_id: 7,
            rows_returned: 42,
        }
    );

    // --- Tear down and inspect ---

    let _ = logger_provider.shutdown();

    println!();
    println!("=== Captured log records ===");
    for log in log_exporter.get_emitted_logs().expect("should get logs") {
        let name = log.record.event_name().unwrap_or("?");
        let severity = log.record.severity_text().unwrap_or("UNKNOWN");
        println!("[{severity}] {name}");
        if let Some(body) = log.record.body() {
            println!("  body: {body:?}");
        }
        for (key, value) in log.record.attributes_iter() {
            println!("  {key} = {value:?}");
        }
        println!();
    }
}
