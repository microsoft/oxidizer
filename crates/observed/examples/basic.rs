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
use observed::{Event, Sink, emit};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

const DC: DataClass = DataClass::new("example", "public");

/// An HTTP request event with two primitive dimensions.
#[derive(Event)]
#[event(name = "http.request")]
#[log(severity = info)]
struct HttpRequest {
    #[data_class(DC)]
    status: i64,
    #[data_class(DC)]
    retries: i64,
}

/// A warning event with a static log message.
#[derive(Event)]
#[event(name = "app.warning")]
#[log(severity = warn, message = "Something unexpected happened")]
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
    // 1. Create an in-memory exporter so we can inspect emitted logs.
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();

    // 2. Build a sink with a processor backed by the logger provider.
    let sink = Sink::new(
        "basic",
        vec![Arc::new(SimpleLogProcessor {
            logger: provider.logger("basic"),
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        tick::SimpleClock::new_system(),
    );

    // 3. Emit events using struct-literal syntax.
    emit!(sink, HttpRequest { status: 200, retries: 0 });
    emit!(sink, AppWarning { code: 42 });

    // 4. Emit using expression syntax (pre-built event).
    let req = HttpRequest { status: 404, retries: 2 };
    emit!(sink, req);

    // 4. Tear down and print captured records.
    let _ = provider.shutdown();

    for log in exporter.get_emitted_logs().expect("should get logs") {
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
