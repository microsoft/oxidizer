// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates processor-level event routing using `EventDescription` flags.
//!
//! Events carry compile-time signal metadata (`is_log`, `contains_metrics`, `is_disabled`)
//! and metric instrument descriptions in their `EventDescription`.
//! Each `EventProcessor` inspects these in `process()` to decide whether
//! to accept the event.
//!
//! This example shows:
//!
//! | Event | Signals | Metric instruments | Who processes it |
//! |-------|---------|--------------------|------------------|
//! | `HttpRequest` | LOG + METRIC | Histogram `http.request.duration` | Log + Metric |
//! | `HttpError` | LOG + METRIC | `Counter` `http.error.count` | Log + Metric |
//! | `MemoryUsage` | METRIC | Gauge `system.memory.usage` | Metric only |
//! | `CacheHit` | LOG | *(none)* | Log only |
//! | `QueueDepth` | DISABLED | *(none)* | Audit (opts in by name) |
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example event_routing
//! ```

use std::sync::{Arc, Mutex};

use observed::metadata::EventDescription;
use observed::processing::EventProcessor;
use observed::{Sink, emit, event};

#[path = "support/redaction.rs"]
mod redaction;

fn main() {
    let log_events: Arc<Mutex<Vec<String>>> = Arc::default();
    let metric_events: Arc<Mutex<Vec<String>>> = Arc::default();
    let audit_events: Arc<Mutex<Vec<String>>> = Arc::default();

    let sink = Sink::new(
        "event_routing",
        vec![
            Arc::new(LogProcessor {
                events: Arc::clone(&log_events),
            }) as Arc<dyn EventProcessor>,
            Arc::new(MetricProcessor {
                events: Arc::clone(&metric_events),
                redaction_engine: redaction::passthrough_redaction_engine(),
            }),
            Arc::new(AuditProcessor {
                allowed_names: &["internal.queue_depth"],
                events: Arc::clone(&audit_events),
            }),
        ],
        tick::SimpleClock::new_system(),
    );

    // --- Emit events ---

    // HttpRequest: LOG + METRIC -> Log processor + Metric processor (Histogram)
    emit!(
        sink,
        HttpRequest {
            status: 200,
            duration_ms: 23.7,
        }
    );

    // HttpError: LOG + METRIC -> Log processor + Metric processor (UpDownCounter)
    emit!(sink, HttpError { route: 1 });

    // MemoryUsage: METRIC only (exclude_from_logs) -> Metric processor only
    emit!(sink, MemoryUsage { bytes: 1_048_576 });

    // CacheHit: LOG only -> Log processor, skipped by Metric processor
    emit!(sink, CacheHit { key_name: 42, items: 3 });

    // QueueDepth: DISABLED -> skipped by Log and Metric, but Audit opts in by name
    emit!(sink, QueueDepth { depth: 17 });

    // --- Print results ---

    println!("Event routing results:");
    println!();

    println!("  Log processor received:");
    for entry in log_events.lock().expect("lock is not poisoned").iter() {
        println!("    {entry}");
    }

    println!();
    println!("  Metric processor received:");
    for entry in metric_events.lock().expect("lock is not poisoned").iter() {
        println!("    {entry}");
    }

    println!();
    println!("  Audit processor received:");
    for entry in audit_events.lock().expect("lock is not poisoned").iter() {
        println!("    {entry}");
    }
}

/// Produces a log record AND a histogram metric for request duration.
#[event("http.request")]
#[info("HTTP request handled")]
#[histogram(duration_ms, name = "http.request.duration")]
struct HttpRequest {
    #[unredacted]
    status: i64,
    #[unredacted]
    duration_ms: f64,
}

/// Produces a log record AND a counter metric for error count.
#[event("http.error")]
#[warning("HTTP errors observed")]
#[counter(name = "http.error.count")]
struct HttpError {
    #[unredacted]
    route: i64,
}

/// Produces a gauge metric only - no log signal.
#[event("system.memory.usage")]
#[gauge(bytes, name = "system.memory.usage")]
struct MemoryUsage {
    #[unredacted]
    bytes: i64,
}

/// A log-only event: no metric fields, so `contains_metrics()` is false.
#[event("cache.hit")]
#[debug("Cache lookup succeeded")]
struct CacheHit {
    #[unredacted]
    key_name: i64,
    #[unredacted]
    items: i64,
}

/// A disabled event: neither a log nor a metric by default.
/// An audit processor can still opt in by matching the event name.
#[event("internal.queue_depth", disabled)]
#[info("Queue depth snapshot")]
struct QueueDepth {
    #[unredacted]
    depth: i64,
}
/// Accepts only log events.
struct LogProcessor {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventProcessor for LogProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.is_log()
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        self.events
            .lock()
            .expect("lock is not poisoned")
            .push(format!("[LOG] {}", event.name()));
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

/// Accepts metric events and records instruments from both metadata paths:
/// field-backed metrics via `FieldDescriptor::metric()` and fieldless metrics
/// via `event.description().metric()`.
struct MetricProcessor {
    events: Arc<Mutex<Vec<String>>>,
    redaction_engine: data_privacy::RedactionEngine,
}

impl EventProcessor for MetricProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.contains_metrics()
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        use std::ops::ControlFlow;

        let engine = &self.redaction_engine;
        self.events
            .lock()
            .expect("lock is not poisoned")
            .push(format!("Event: {}", event.name()));

        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(metric) = desc.metric()
                && let Some(instrument) = metric.instrument_description()
            {
                let value = get_value(engine);
                self.events.lock().expect("lock is not poisoned").push(format!(
                    "  [METRIC] {}({}) = {value}",
                    instrument.kind(),
                    instrument.instrument_name(),
                ));
            }
            ControlFlow::Continue(())
        });

        if let Some(metric) = event.description().metric() {
            self.events.lock().expect("lock is not poisoned").push(format!(
                "  [METRIC] {}({}) = 1",
                metric.kind(),
                metric.instrument_name()
            ));
        }
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

/// An audit processor that opts in to specific disabled events by name.
/// Demonstrates that `is_disabled` doesn't prevent processing - the processor decides.
struct AuditProcessor {
    allowed_names: &'static [&'static str],
    events: Arc<Mutex<Vec<String>>>,
}

impl EventProcessor for AuditProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.allowed_names.contains(&description.name())
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        self.events
            .lock()
            .expect("lock is not poisoned")
            .push(format!("[AUDIT] {}", event.name()));
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}
