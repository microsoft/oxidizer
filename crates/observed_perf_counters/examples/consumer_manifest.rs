// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints the exact installer input for the opt-in Perflib consumer integration test.
//! No provider is started and no system schema is installed.

use observed::event;
use observed_perf_counters::PerfCounterProcessor;

#[event("request.completed")]
#[counter(name = "Requests")]
#[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
}

#[event("huge.increment")]
#[counter(value, name = "Huge")]
struct HugeIncrement {
    #[unredacted]
    value: u64,
}

#[event("disabled", disabled)]
#[counter(bytes_sent, name = "Disabled", unit = "By")]
struct Disabled {
    #[unredacted]
    bytes_sent: u32,
}

#[event("requests.active")]
#[updown_counter(delta, name = "Active Requests")]
struct ActiveRequests {
    #[unredacted]
    delta: i32,
}

#[event("queue.depth")]
#[gauge(depth, name = "Queue Depth")]
struct QueueDepth {
    #[unredacted]
    depth: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PerfCounterProcessor::builder()
        .category("Observed Windows consumer test <&\"'> 中文🦀")
        .instance("worker")
        .bind_event::<RequestCompleted>()
        .bind_event::<HugeIncrement>()
        .bind_event::<Disabled>()
        .bind_event::<ActiveRequests>()
        .bind_event::<QueueDepth>()
        .manifest("ObservedWindowsTestCounters.dll")?;
    println!("{manifest}");
    Ok(())
}
