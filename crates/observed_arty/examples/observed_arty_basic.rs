// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transfers request enrichment to a task on another Arty worker.
//!
//! Run with:
//! ```sh
//! cargo run -p observed_arty --example observed_arty_basic
//! ```

use std::ops::ControlFlow;
use std::sync::Arc;

use arty::runtime::Runtime;
use arty::runtime::policy::WorkersPolicy;
use observed::enrichment::EnrichFutureExt as _;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{Enrichment, FlushError, Sink, emit, event};
use observed_arty::ObservedExt as _;

#[derive(Enrichment)]
struct RequestContext {
    #[dimension(log = "request.id")]
    #[unredacted]
    request_id: &'static str,
}

#[event("example.task")]
#[info("Arty task observed")]
struct TaskObserved;

struct PrintingProcessor;

impl EventProcessor for PrintingProcessor {
    fn is_interested(&self, _description: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let mut enrichment_names = Vec::new();
        let _ = event.visit_enrichments(&mut |description, _get_value| {
            enrichment_names.push(description.field_name());
            ControlFlow::Continue(())
        });
        println!("{} enrichment: {}", event.name(), enrichment_names.join(", "));
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sink = Sink::builder("example").processors(vec![Arc::new(PrintingProcessor)]).build();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .enable_observed(&sink)
        .build()?;

    runtime.scheduler().block_on(async move |cx| {
        let target = cx
            .control()
            .workers()
            .into_iter()
            .find(|worker| worker.id() != cx.thread().id())
            .expect("the runtime has two distinct workers")
            .clone();
        let child_sink = sink.clone();

        async {
            cx.scheduler()
                .spawn_at(&target, child_sink, |sink| async move {
                    emit!(sink, TaskObserved);
                })
                .await
                .expect("the child task must complete");
        }
        .enrich(&sink, RequestContext { request_id: "request-42" })
        .await;
    })?;

    runtime.stop()?;
    Ok(())
}
