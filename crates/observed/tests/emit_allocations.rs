// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation contracts for emitting events.

#![cfg(not(miri))]

use std::alloc::System;
use std::sync::Arc;

use alloc_tracker::{Allocator, Session};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{FlushError, Sink, emit, event};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

#[event("request.handled")]
#[info]
struct RequestHandled {
    #[unredacted]
    status: i64,
}

struct DiscardingProcessor;

impl EventProcessor for DiscardingProcessor {
    fn is_interested(&self, _description: &EventDescription) -> bool {
        true
    }

    fn process(&self, _event: &EventView<'_>) {}

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

#[test]
fn repeated_emit_to_sink_with_many_processors_does_not_allocate() {
    let processors = (0..64).map(|_| Arc::new(DiscardingProcessor) as Arc<dyn EventProcessor>).collect();
    let sink = Sink::new("many", processors, tick::SimpleClock::new_frozen());
    emit!(sink, RequestHandled { status: 200 });

    let session = Session::new().no_stdout().no_file();
    {
        let _span = session.operation("emit").measure_thread().iterations(1);
        emit!(sink, RequestHandled { status: 200 });
    }

    let allocations: u64 = session
        .to_report()
        .operations()
        .map(|(_name, operation)| operation.total_allocations_count())
        .sum();
    assert_eq!(allocations, 0);
}
