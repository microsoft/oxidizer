// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Optional event fields.
//!
//! A field of type `Option<T>` is captured like a `T` when it is `Some(_)`. When
//! it is `None`, `#[if_none(...)]` decides what happens:
//!
//! - the default (`#[if_none("n/a")]`) records a stable `"n/a"` placeholder for
//!   the log attribute and/or metric dimension, keeping a fixed schema;
//! - `#[if_none("...")]` records a custom placeholder instead (e.g. `"unknown"`);
//! - `#[if_none(drop)]` omits the field entirely, so neither a log
//!   attribute nor a metric dimension is recorded for that emission.
//!
//! The distinction is most visible for **metric dimensions**, where dropping a
//! dimension changes the shape of the time series while filling keeps it stable.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example optional_fields
//! ```

use std::sync::Arc;

use data_privacy::classified;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{Event, Sink, emit};
use observed_testing::MicrosoftEnterpriseDataTaxonomy as DataTaxonomy;

fn main() {
    // A passthrough redaction engine keeps classified values readable for this
    // example; a real deployment would configure redactors per data class.
    let redaction_engine = data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
            data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
        ))
        .build();

    let sink = Sink::new(
        "optional_fields",
        vec![Arc::new(PrintProcessor { redaction_engine })],
        tick::SimpleClock::new_system(),
    );

    // Everything known: both optional fields are `Some`, so both are recorded as-is.
    emit!(
        sink,
        HttpResponse {
            method: HttpMethod("GET".to_owned()),
            status: HttpStatus(200),
            duration_ms: DurationMs(42.7),
            region: Some(Region("westus".to_owned())),
            cache_status: Some(CacheStatus("hit".to_owned())),
        }
    );

    // Everything unknown: both optional fields are `None`.
    // - `region` uses `#[if_none("unknown")]`, so it is recorded as `"unknown"`
    //   (log and metric dimension), keeping a stable schema.
    // - `cache_status` uses `#[if_none(drop)]`, so it is omitted entirely.
    emit!(
        sink,
        HttpResponse {
            method: HttpMethod("POST".to_owned()),
            status: HttpStatus(503),
            duration_ms: DurationMs(1280.0),
            region: None,
            cache_status: None,
        }
    );
}

// Classified newtypes carry their data classification in the type system, so
// fields don't need a separate `#[data_class(...)]` annotation.

#[classified(DataTaxonomy::SystemMetadata)]
#[derive(Clone)]
struct HttpMethod(String);

#[classified(DataTaxonomy::SystemMetadata)]
#[derive(Clone, Copy)]
struct HttpStatus(u16);

#[classified(DataTaxonomy::SystemMetadata)]
#[derive(Clone, Copy)]
struct DurationMs(f64);

#[classified(DataTaxonomy::SystemMetadata)]
#[derive(Clone)]
struct Region(String);

#[classified(DataTaxonomy::SystemMetadata)]
#[derive(Clone)]
struct CacheStatus(String);

/// An HTTP response event with two optional fields that behave differently when
/// absent.
#[derive(Event)]
#[event(name = "http.response")]
#[log(severity = info,
      message = "{http.request.method} -> {http.response.status}")]
#[metric(kind = histogram, field = status)]
struct HttpResponse {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    method: HttpMethod,

    #[dimension(log = "http.response.status")]
    status: HttpStatus,

    #[dimension(log = "http.server.duration")]
    duration_ms: DurationMs,

    #[dimension(metric = "region")]
    #[if_none("unknown")]
    region: Option<Region>,

    #[dimension(log = "http.response.cache_status")]
    #[if_none(drop)]
    cache_status: Option<CacheStatus>,
}

/// A minimal processor that prints each event's log attributes straight to
/// stdout.
struct PrintProcessor {
    redaction_engine: data_privacy::RedactionEngine,
}

impl EventProcessor for PrintProcessor {
    fn is_interested(&self, _description: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        use std::ops::ControlFlow;

        let severity = event.severity().map_or("UNKNOWN", |s| s.as_str());
        println!("[{severity}] {name}", name = event.name());

        let engine = &self.redaction_engine;
        let _ = event.visit_fields(&mut |desc, get_value| {
            // Only fields that survived `Option` routing reach here.
            if let Some(log) = desc.log() {
                println!("  {key} = {value}", key = log.key(), value = get_value(engine));
            }
            ControlFlow::Continue(())
        });
        println!();
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}
