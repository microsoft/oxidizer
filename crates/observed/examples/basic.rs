// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Simplest possible `observed` usage: define events, register a sink, and emit them.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example basic
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

/// An HTTP request event with two primitive dimensions.
#[event("http.request")]
#[info]
struct HttpRequest {
    #[data_class(DC)]
    status: i64,
    #[data_class(DC)]
    retries: i64,
}

/// A warning event with a static log message.
#[event("app.warning")]
#[warning("Something unexpected happened")]
struct AppWarning {
    #[data_class(DC)]
    code: i64,
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

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

fn main() {
    // 1. Create an in-memory exporter so we can inspect emitted logs.
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();

    // 2. Build a sink with a processor backed by the logger provider.
    let sink = Sink::new(
        "basic",
        vec![Arc::new(SimpleLogProcessor {
            logger: provider.logger("basic"),
            redaction_engine: redaction::passthrough_redaction_engine(),
        })],
        tick::SimpleClock::new_system(),
    );

    // 3. Emit events using struct-literal syntax.
    emit!(sink, HttpRequest { status: 200, retries: 0 });
    emit!(sink, AppWarning { code: 42 });

    // 4. Emit using expression syntax (pre-built event).
    let req = HttpRequest { status: 404, retries: 2 };
    emit!(sink, req);

    // 4. Read the captured records, then tear down.
    //    Read emitted logs *before* shutdown because OTel 0.32's
    //    `InMemoryLogExporter` clears its buffer on shutdown by default.
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();

    for log in logs {
        println!(
            "[{severity}] {name}",
            severity = log.record.severity_text().unwrap_or("UNKNOWN"),
            name = log.record.event_name().unwrap_or("?"),
        );
        for (key, value) in log.record.attributes_iter() {
            println!("  {key} = {value:?}");
        }
        if let Some(body) = log.record.body() {
            println!("  body = {body:?}");
        }
        println!();
    }
}
