// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates the Sink processor model with per-field routing.
//!
//! Events flow through a [`Sink`] that dispatches to one or more
//! [`EventProcessor`]s. Each processor has its own redaction engine and
//! applies it lazily while extracting only the field and enrichment values it
//! exports.
//!
//! Field routing (which fields go to logs vs metric dimensions) is declared
//! inside `#[event(...)]` attributes. Fields are log attributes by default;
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
use observed::{Sink, emit, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

#[path = "support/redaction.rs"]
mod redaction;

#[path = "support/otel.rs"]
mod otel;

const DC: DataClass = DataClass::new("example", "public");

// ---------------------------------------------------------------------------
// Event types with declarative routing via #[event(...)] attributes
// ---------------------------------------------------------------------------

/// An HTTP request event with **declarative** log and metric routing.
///
/// Every field is a log attribute by default. Use `#[dimension(metric = "...")]`
/// to opt a field into metric dimensions, or `#[dimension(log = exclude)]` to
/// drop it from logs.
#[event("http.server.request")]
#[info("HTTP request completed")]
#[histogram(duration_ms, name = "http.server.request.duration")]
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
#[event("db.query")]
#[info]
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
        use opentelemetry::logs::Logger;

        let mut record = self.logger.create_log_record();
        if otel::populate_log_record(&mut record, event, &self.redaction_engine) {
            self.logger.emit(record);
        }
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
            redaction_engine: redaction::passthrough_redaction_engine(),
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

    // --- Read the captured records, then tear down ---

    // Read emitted logs *before* shutdown because OTel 0.32's
    // `InMemoryLogExporter` clears its buffer on shutdown by default.
    let _ = logger_provider.force_flush();
    let logs = log_exporter.get_emitted_logs().expect("should get logs");
    let _ = logger_provider.shutdown();

    println!();
    println!("=== Captured log records ===");
    for log in logs {
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
