// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implements and attaches an event sampler while preserving borrowed events.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example event_sampling
//! ```

use std::sync::Arc;

use data_privacy::{DataClass, Sensitive};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{EventSampler, EventSamplingContext, EventSamplingDecision, FlushError, Sink, emit, event};

const PUBLIC_DATA: DataClass = DataClass::new("example", "public");

#[event("request.completed")]
#[info("Request completed")]
#[histogram(duration_ms, name = "request.duration_ms")]
struct RequestCompleted<'a> {
    route: Sensitive<&'a str>,
    #[unredacted]
    duration_ms: f64,
}

#[event("health.check")]
#[info("Health check")]
#[counter(name = "health.check.count")]
struct HealthCheck;

#[derive(Clone, Copy)]
enum Signal {
    Log,
    Metric,
}

impl Signal {
    const fn label(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Metric => "metric",
        }
    }
}

struct PrintingProcessor {
    signal: Signal,
}

impl EventProcessor for PrintingProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        match self.signal {
            Signal::Log => description.is_log(),
            Signal::Metric => description.contains_metrics(),
        }
    }

    fn process(&self, event: &EventView<'_>) {
        println!("{} {}", self.signal.label(), event.name());
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

struct DropHealthChecks;

impl EventSampler for DropHealthChecks {
    fn sample(&self, event: &EventSamplingContext<'_>) -> EventSamplingDecision {
        let decision = if event.description().name() == "health.check" {
            EventSamplingDecision::Drop
        } else {
            EventSamplingDecision::Continue
        };
        println!(
            "sample {} on {} at {:?} -> {decision:?}",
            event.description().name(),
            event.sink_id(),
            event.timestamp(),
        );

        decision
    }
}

fn main() {
    let processors: Vec<Arc<dyn EventProcessor>> = vec![
        Arc::new(PrintingProcessor { signal: Signal::Log }),
        Arc::new(PrintingProcessor { signal: Signal::Metric }),
    ];
    let sink = Sink::new("service", processors, tick::SimpleClock::new_system()).with_event_sampler(Arc::new(DropHealthChecks));

    let route = String::from("/users");
    emit!(
        sink,
        RequestCompleted {
            route: Sensitive::new(route.as_str(), PUBLIC_DATA),
            duration_ms: 12.5,
        }
    );
    emit!(sink, HealthCheck);
}
