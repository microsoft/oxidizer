// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "example code")]

//! Demonstrates event identification in processors using two approaches:
//!
//! 1. **`TypeId` matching** - type-safe, works with generics, but only for
//!    compile-time events (`#[derive(Event)]`). `TypeId` is `None` for dynamic
//!    events (e.g. from the tracing bridge).
//!
//! 2. **Name matching** - uses the canonical `#[event(name = "...")]` string.
//!    Works for all events (compile-time and dynamic), easy to configure from
//!    external data, but not type-checked at compile time.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example event_type_matching
//! ```

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use data_privacy::DataClass;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{Event, Sink, emit};

fn main() {
    let routing_log: Arc<Mutex<Vec<String>>> = Arc::default();
    let http_log: Arc<Mutex<Vec<String>>> = Arc::default();
    let name_log: Arc<Mutex<Vec<String>>> = Arc::default();

    let sink = Sink::new(
        "event_type_matching",
        vec![
            Arc::new(TypeRoutingProcessor::new(Arc::clone(&routing_log))) as Arc<dyn EventProcessor>,
            Arc::new(HttpOnlyProcessor {
                log: Arc::clone(&http_log),
                engine: passthrough_engine(),
            }),
            Arc::new(NameRoutingProcessor::new(
                // Accept only events whose name starts with "http."
                &["http.request", "http.error"],
                Arc::clone(&name_log),
            )),
        ],
        tick::SimpleClock::new_system(),
    );

    emit!(sink, HttpRequest { status: 200 });
    let msg = String::from("hello");
    emit!(sink, BorrowedEvent { message: &msg });
    emit!(sink, GenericEvent { value: 42i64 });
    emit!(
        sink,
        GenericEvent {
            value: String::from("payload"),
        }
    );
    emit!(sink, AppError { code: 500 });
    emit!(sink, LifecycleReady {});

    // --- Print results ---

    println!("=== TypeId matching ===");
    println!();

    println!("TypeRoutingProcessor handled:");
    for entry in routing_log.lock().unwrap().iter() {
        println!("  {entry}");
    }

    println!();
    println!("HttpOnlyProcessor received (prefiltered via is_interested):");
    for entry in http_log.lock().unwrap().iter() {
        println!("  {entry}");
    }

    println!();
    println!("=== Name matching ===");
    println!();

    println!("NameRoutingProcessor received (allowed: http.request, http.error):");
    for entry in name_log.lock().unwrap().iter() {
        println!("  {entry}");
    }
}

const DC: DataClass = DataClass::new("example", "public");

/// Creates a passthrough redaction engine that allows all data classes.
fn passthrough_engine() -> data_privacy::RedactionEngine {
    data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
            data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
        ))
        .build()
}

/// Concrete struct - the common case
#[derive(Event)]
#[event(name = "http.request")]
#[log(severity = info)]
struct HttpRequest {
    #[data_class(DC)]
    status: i64,
}

/// Struct with a lifetime parameter
#[derive(Event)]
#[event(name = "borrowed.message")]
#[log(severity = info)]
struct BorrowedEvent<'a> {
    #[data_class(DC)]
    message: &'a str,
}

/// Generic struct
#[derive(Event)]
#[event(name = "generic.payload")]
#[log(severity = info)]
struct GenericEvent<T>
where
    T: Clone + Send + Sync,
    observed::Value: From<T>,
{
    #[unredacted]
    value: T,
}

/// Another concrete struct to show negative matching
#[derive(Event)]
#[event(name = "app.error")]
#[log(severity = warn)]
struct AppError {
    #[data_class(DC)]
    code: i64,
}

/// An event with no signal annotations - neither `#[log]` nor metric attributes.
/// Still a valid event: it can be matched by `TypeId` or name, but produces
/// no log or metric output and `visit_fields` yields nothing.
#[derive(Event)]
#[event(name = "lifecycle.ready")]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "the Event derive operates on structs with named fields"
)]
struct LifecycleReady {}

type EventHandler = Box<dyn Fn(&EventView<'_>, &data_privacy::RedactionEngine) + Send + Sync>;

/// A processor that routes events to handlers based on their `TypeId`.
struct TypeRoutingProcessor {
    handlers: HashMap<TypeId, EventHandler>,
    engine: data_privacy::RedactionEngine,
}

impl TypeRoutingProcessor {
    #[expect(clippy::needless_pass_by_value, reason = "Arc is cheap to clone; by-value is idiomatic here")]
    fn new(log: Arc<Mutex<Vec<String>>>) -> Self {
        let mut handlers = HashMap::<TypeId, EventHandler>::new();

        let http_log = Arc::clone(&log);
        handlers.insert(
            EventDescription::type_id_of::<HttpRequest>().unwrap(),
            Box::new(move |event, engine| {
                let fields = collect_fields(event, engine);
                http_log.lock().unwrap().push(format!("HttpRequest {{{}}}", format_fields(&fields)));
            }),
        );

        let borrowed_log = Arc::clone(&log);
        handlers.insert(
            EventDescription::type_id_of::<BorrowedEvent<'_>>().unwrap(),
            Box::new(move |event, engine| {
                let fields = collect_fields(event, engine);
                borrowed_log
                    .lock()
                    .unwrap()
                    .push(format!("BorrowedEvent {{{}}}", format_fields(&fields)));
            }),
        );

        let generic_i64_log = Arc::clone(&log);
        handlers.insert(
            EventDescription::type_id_of::<GenericEvent<i64>>().unwrap(),
            Box::new(move |event, engine| {
                let fields = collect_fields(event, engine);
                generic_i64_log
                    .lock()
                    .unwrap()
                    .push(format!("GenericEvent<i64> {{{}}}", format_fields(&fields)));
            }),
        );

        let generic_string_log = Arc::clone(&log);
        handlers.insert(
            EventDescription::type_id_of::<GenericEvent<String>>().unwrap(),
            Box::new(move |event, engine| {
                let fields = collect_fields(event, engine);
                generic_string_log
                    .lock()
                    .unwrap()
                    .push(format!("GenericEvent<String> {{{}}}", format_fields(&fields)));
            }),
        );

        // No-signal event: matched by TypeId, but visit_fields yields nothing
        // for logs since there is no `#[log]` annotation.
        let lifecycle_log = Arc::clone(&log);
        handlers.insert(
            EventDescription::type_id_of::<LifecycleReady>().unwrap(),
            Box::new(move |event, engine| {
                let fields = collect_fields(event, engine);
                lifecycle_log
                    .lock()
                    .unwrap()
                    .push(format!("LifecycleReady {{{}}}", format_fields(&fields)));
            }),
        );

        Self {
            handlers,
            engine: passthrough_engine(),
        }
    }
}

impl EventProcessor for TypeRoutingProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.type_id().is_some_and(|id| self.handlers.contains_key(&id))
    }

    fn process(&self, event: &EventView<'_>) {
        // is_interested guarantees the event has a TypeId and a handler
        let type_id = event.description().type_id().unwrap();
        let handler = self.handlers.get(&type_id).unwrap();
        handler(event, &self.engine);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// A processor that only accepts `HttpRequest` events by checking the event's `TypeId` in `is_interested`.
struct HttpOnlyProcessor {
    log: Arc<Mutex<Vec<String>>>,
    engine: data_privacy::RedactionEngine,
}

impl EventProcessor for HttpOnlyProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.type_id() == EventDescription::type_id_of::<HttpRequest>()
    }

    fn process(&self, event: &EventView<'_>) {
        let fields = collect_fields(event, &self.engine);
        self.log.lock().unwrap().push(format!("HttpRequest {{{}}}", format_fields(&fields)));
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// A processor that accepts events by matching their canonical event name
/// (from `#[event(name = "...")]`).
///
/// Unlike `TypeId` matching this works for dynamic events too, and the set of
/// accepted names can be loaded from configuration at runtime.
struct NameRoutingProcessor {
    allowed: HashSet<&'static str>,
    log: Arc<Mutex<Vec<String>>>,
    engine: data_privacy::RedactionEngine,
}

impl NameRoutingProcessor {
    fn new(names: &[&'static str], log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            allowed: names.iter().copied().collect(),
            log,
            engine: passthrough_engine(),
        }
    }
}

impl EventProcessor for NameRoutingProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.allowed.contains(description.name())
    }

    fn process(&self, event: &EventView<'_>) {
        let fields = collect_fields(event, &self.engine);
        self.log
            .lock()
            .unwrap()
            .push(format!("{} {{{}}}", event.name(), format_fields(&fields)));
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// Collects all log-routed fields from an event into `(key, value)` pairs.
fn collect_fields(event: &EventView<'_>, engine: &data_privacy::RedactionEngine) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let _ = event.visit_fields(&mut |desc, get| {
        if let Some(log) = desc.log() {
            fields.push((log.key().to_owned(), get(engine).to_string()));
        }
        ControlFlow::Continue(())
    });
    fields
}

/// Formats collected fields as `key1=val1, key2=val2`.
fn format_fields(fields: &[(String, String)]) -> String {
    fields.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ")
}
