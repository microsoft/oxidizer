// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Publish using an externally authored .man until Enter is pressed.
//! Pass the manifest path as the first argument; no schema is installed.

use std::env::args;
use std::io::stdin;
use std::path::PathBuf;
use std::process::id;

use observed::{Sink, emit, event};
use observed_perf_counters::PerfCounterProcessor;
use performables::arc::Arc;
use tick::SimpleClock;

#[event("request.completed")]
#[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
struct RequestCompleted {
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

fn main() -> Result<(), ohno::AppError> {
    let manifest = args().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join("MyServiceCounters.man")
        },
        PathBuf::from,
    );
    let processor = PerfCounterProcessor::builder()
        .manifest_file(manifest)?
        .category("MyService")
        .instance(format!("worker-{}", id()))
        .bind_event::<RequestCompleted>()
        .bind_event::<ActiveRequests>()
        .bind_event::<QueueDepth>()
        .build()?;
    let sink = Sink::builder("app")
        .processors(vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::new(processor))])
        .clock(SimpleClock::new_system())
        .build();
    emit!(sink, ActiveRequests { delta: 1 });
    emit!(sink, RequestCompleted { bytes_sent: 1024 });
    emit!(sink, RequestCompleted { bytes_sent: 2048 });
    emit!(sink, QueueDepth { depth: 10 });
    emit!(sink, QueueDepth { depth: 7 });
    emit!(sink, ActiveRequests { delta: -1 });
    println!("Publishing 3072 Bytes Sent, 0 Active Requests, and Queue Depth 7; press Enter to stop.");
    let mut line = String::new();
    stdin().read_line(&mut line)?;
    sink.flush()?;
    Ok(())
}
