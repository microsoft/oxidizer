// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation contract for valid integer metric emission.

#![cfg(all(windows, target_pointer_width = "64", not(miri)))]

use std::path::PathBuf;

use alloc_tracker::{Allocator, Session};
use observed::{Sink, emit, event};
use observed_perf_counters::PerfCounterProcessor;
use performables::arc::Arc;
use tick::SimpleClock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

testing_aids::init_tracing!();

#[event("metrics.allocation")]
#[counter(bytes, name = "Bytes")]
#[updown_counter(active, name = "Active")]
#[gauge(depth, name = "Depth")]
struct Measurements {
    #[unredacted]
    bytes: u32,
    #[unredacted]
    active: i32,
    #[unredacted]
    depth: u32,
}

#[test]
fn valid_counter_updown_and_gauge_updates_allocate_nothing() {
    let processor = PerfCounterProcessor::builder()
        .manifest_file(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("test_data")
                .join("ConsumerCounters.man"),
        )
        .unwrap()
        .category("IntegerMetrics")
        .instance("worker")
        .bind_event::<Measurements>()
        .build()
        .unwrap();
    let sink = Sink::new(
        "allocations",
        vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::new(processor))],
        SimpleClock::new_frozen(),
    );
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("integer_emission");
    {
        let _measurement = operation.measure_thread().iterations(200);
        for _ in 0..100 {
            emit!(
                sink,
                Measurements {
                    bytes: 37,
                    active: 1,
                    depth: 10
                }
            );
            emit!(
                sink,
                Measurements {
                    bytes: 0,
                    active: -1,
                    depth: 7
                }
            );
        }
    }
    sink.flush().unwrap();
    let allocated = session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == "integer_emission").then(|| operation.total_bytes_allocated()))
        .unwrap();
    assert_eq!(allocated, 0);
}
