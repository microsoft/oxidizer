// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies that `observed` works correctly with a multithreaded Tokio runtime
//! (the default `#[tokio::main]` configuration).
//!
//! This example spawns concurrent tasks that emit events with enrichment,
//! demonstrating that context propagation via `EnrichFutureExt::enrich`
//! (which wraps the future so enrichment is pushed on every poll) survives
//! cross-thread `.await` points.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example tokio_multithread
//! ```

use std::sync::Arc;

use data_privacy::DataClass;
use observed::enrichment::EnrichFutureExt;
use observed::{Enrichment, Event, Sink, emit};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

const DC: DataClass = DataClass::new("example", "public");

#[derive(Debug, Clone, Enrichment)]
struct ServiceContext {
    #[data_class(DC)]
    service: i64,
}

#[derive(Event)]
#[event(name = "task.completed")]
#[log(severity = info, message = "Task completed")]
struct TaskCompleted {
    #[data_class(DC)]
    task_id: i64,
    #[data_class(DC)]
    result: i64,
}

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

#[tokio::main]
async fn main() {
    // 1. Set up an in-memory exporter so we can inspect emitted logs.
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();

    let sink = Sink::new(
        "tokio_multithread",
        vec![Arc::new(SimpleLogProcessor {
            logger: provider.logger("tokio_mt"),
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        tick::SimpleClock::new_system(),
    );

    // 2. Spawn tasks inside an enrichment scope - each task wraps its future
    //    with `.enrich()` so the enrichment survives cross-thread migrations.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));
    let mut handles = Vec::new();

    for task_id in 0..4 {
        let b = std::sync::Arc::clone(&barrier);
        let task_emitter = sink.clone();
        handles.push(tokio::spawn(
            async move {
                // Synchronize so all tasks are running concurrently.
                b.wait().await;

                // Yield several times to allow thread migration.
                for i in 0..3 {
                    tokio::task::yield_now().await;
                    emit!(task_emitter, TaskCompleted { task_id, result: i });
                }
            }
            .enrich(&sink, ServiceContext { service: 1 }),
        ));
    }

    // 3. Await all spawned tasks.
    for handle in handles {
        handle.await.expect("task should not panic");
    }

    // 4. Flush and verify.
    //    Read emitted logs *before* shutdown because OTel 0.32's
    //    `InMemoryLogExporter` clears its buffer on shutdown by default.
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    assert_eq!(logs.len(), 12, "expected 12 logs (4 tasks x 3 emits each), got {}", logs.len());

    // Verify every log carries the enrichment from the outer scope.
    for log in &logs {
        let has_service = log.record.attributes_iter().any(|(k, _)| k.as_str() == "service");
        assert!(has_service, "enrichment 'service' should be present on every log");
    }

    println!(
        "SUCCESS: emitted {} logs across 4 concurrent Tokio tasks with enrichment propagation.",
        logs.len()
    );
    for log in &logs {
        println!(
            "  [{severity}] {name}",
            severity = log.record.severity_text().unwrap_or("UNKNOWN"),
            name = log.record.event_name().unwrap_or("?"),
        );
        for (key, value) in log.record.attributes_iter() {
            println!("    {key} = {value:?}");
        }
    }
}
