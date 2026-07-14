// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows how `enriched_spawner` propagates enrichments into spawned tasks.
//!
//! Run with:
//! ```sh
//! cargo run -p observed_rt --example spawner
//! ```

use std::sync::Arc;

use data_privacy::DataClass;
use observed::enrichment::EnrichFutureExt;
use observed::{Enrichment, Event, Sink, emit};
use opentelemetry_sdk::logs::SdkLoggerProvider;

const DC: DataClass = DataClass::new("example", "public");

#[derive(Event)]
#[event(name = "task.ping")]
#[log(severity = info, message = "ping from spawned task")]
struct Ping {
    #[data_class(DC)]
    task_id: i64,
}

#[derive(Enrichment)]
struct RequestCtx {
    #[dimension(log = "request.id")]
    #[data_class(DC)]
    request_id: i64,
}

#[tokio::main]
async fn main() {
    let sink = setup_stdout_exporter();

    // Wrap a Tokio spawner so spawned tasks inherit the caller's context.
    let spawner = observed_rt::tokio(&sink);
    let spawn_emitter = sink.clone();
    async {
        spawner
            .spawn(async move {
                // this event will contain `request.id` enrichment
                emit!(spawn_emitter, Ping { task_id: 1 });
            })
            .await;
    }
    .enrich(&sink, RequestCtx { request_id: 42 })
    .await;
}

struct SimpleLogProcessor {
    logger_provider: SdkLoggerProvider,
    redaction_engine: data_privacy::RedactionEngine,
}

impl observed::processing::EventProcessor for SimpleLogProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        use std::ops::ControlFlow;

        use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider};

        let logger = self.logger_provider.logger(event.name());
        let mut record = logger.create_log_record();
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
        logger.emit(record);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// Set up an sink that prints logs to stdout.
fn setup_stdout_exporter() -> Sink {
    let provider = SdkLoggerProvider::builder()
        .with_simple_exporter(opentelemetry_stdout::LogExporter::default())
        .build();
    Sink::new(
        "spawner",
        vec![Arc::new(SimpleLogProcessor {
            logger_provider: provider,
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        tick::SimpleClock::new_system(),
    )
}
