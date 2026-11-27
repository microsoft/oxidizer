// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Run async tasks, blocking tasks, and allocations for a Seismograph showcase.
//!
//! Start the workload:
//!
//! ```text
//! cargo run -p arty --example arty_monitoring
//! ```
//!
//! Start the monitor in another terminal:
//!
//! ```text
//! cargo run -p seismograph_cli -- monitor
//! ```
//!
//! The workload runs until you stop it. The Anvil example check builds the
//! example, prints a skip message, and exits without running the workload.

#![forbid(unsafe_code)]

use std::env;
use std::io::Write as _;
use std::sync::OnceLock;
use std::time::Duration;

use arty::task::{Context, JoinError};
use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::io::{BufferId, IoEvent, IoOperationId, IoOutcome, IoResourceId, IoResourceKind};

rallocator::rallocator!(backend = dynamic, default = system);

const ASYNC_STEPS: usize = 8;
const ALLOCATIONS_PER_STEP: usize = 64;
const ALLOCATION_SIZE: usize = 8 * 1024;
const BLOCKING_ALLOCATION_COUNT: usize = 32;
const BLOCKING_ALLOCATION_SIZE: usize = 16 * 1024;
const IO_PAYLOAD_SIZE: usize = 64 * 1024;
const SESSION_COUNT: usize = 256;
const ANALYSIS_SAMPLE_COUNT: usize = 250_000;

const ASYNC_STEP_DELAY: Duration = Duration::from_millis(250);
const BLOCKING_TASK_DELAY: Duration = Duration::from_millis(750);
const BLOCKING_ALLOCATION_DELAY: Duration = Duration::from_millis(1_250);
const CYCLE_DELAY: Duration = Duration::from_millis(500);

struct CycleSummary {
    allocated_bytes: usize,
    active_sessions: usize,
    cache_hits: usize,
    io_bytes: usize,
    percentile: u64,
}

#[arty::main(workers = 2, monitor = enabled)]
async fn main(cx: Context) -> Result<(), JoinError> {
    if env::var_os("ANVIL_EXAMPLE").is_some() {
        println!("Arty monitoring is an interactive example and does not run during automated builds.");
        return Ok(());
    }

    let mut cycle = 0_u64;

    println!("Arty monitoring showcase started. Open seismograph_cli and select this process.");

    loop {
        cycle += 1;
        let cycle_task = cx.scheduler().spawn(move |cx| async move { run_cycle(cx, cycle).await });
        let summary = cycle_task.await??;

        println!(
            "cycle {cycle}: allocated {} KiB, retained {} sessions, cache hits {}, I/O {} KiB, percentile sample {}",
            summary.allocated_bytes / 1024,
            summary.active_sessions,
            summary.cache_hits,
            summary.io_bytes / 1024,
            summary.percentile
        );

        cx.clock().delay(CYCLE_DELAY).await;
    }
}

async fn run_cycle(cx: Context, cycle: u64) -> Result<CycleSummary, JoinError> {
    let allocation_task = cx
        .scheduler()
        .spawn(move |cx| async move { run_allocation_bursts(cx, cycle).await });
    let session_task = cx.scheduler().spawn(move |cx| async move { churn_sessions(cx, cycle).await });
    let cache_task = cx.scheduler().spawn(move |_| async move { run_cache_workload(cycle) });
    let analysis_task = cx.scheduler().spawn_blocking(move || run_blocking_analysis(cycle));
    let blocking_allocation_task = cx.scheduler().spawn_blocking(move || run_blocking_allocations(cycle));
    let io_task = cx.scheduler().spawn_blocking(move || run_file_io(cycle));

    Ok(CycleSummary {
        allocated_bytes: allocation_task.await? + blocking_allocation_task.await?,
        active_sessions: session_task.await?,
        cache_hits: cache_task.await?,
        io_bytes: io_task.await?,
        percentile: analysis_task.await?,
    })
}

async fn run_allocation_bursts(cx: Context, cycle: u64) -> usize {
    let mut allocated_bytes = 0;

    for step in 0..ASYNC_STEPS {
        let fill = cycle.wrapping_add(step as u64) as u8;
        let payloads: Vec<_> = (0..ALLOCATIONS_PER_STEP)
            .map(|allocation| vec![fill.wrapping_add(allocation as u8); ALLOCATION_SIZE])
            .collect();
        allocated_bytes += payloads.iter().map(Vec::len).sum::<usize>();

        cx.clock().delay(ASYNC_STEP_DELAY).await;
        std::hint::black_box(payloads);
    }

    allocated_bytes
}

async fn churn_sessions(cx: Context, cycle: u64) -> usize {
    let mut sessions: Vec<_> = (0..SESSION_COUNT).map(|session| format!("session-{cycle}-{session}")).collect();

    cx.clock().delay(ASYNC_STEP_DELAY).await;
    sessions.retain(|session| session.len().is_multiple_of(2));
    cx.clock().delay(ASYNC_STEP_DELAY).await;

    sessions.len()
}

fn run_blocking_analysis(cycle: u64) -> u64 {
    let mut samples: Vec<_> = (0..ANALYSIS_SAMPLE_COUNT as u64)
        .map(|sample| sample.wrapping_mul(31).wrapping_add(cycle) % 10_000)
        .collect();
    samples.sort_unstable();

    std::thread::sleep(BLOCKING_TASK_DELAY);

    samples[samples.len() * 95 / 100]
}

fn run_blocking_allocations(cycle: u64) -> usize {
    let allocations: Vec<_> = (0..BLOCKING_ALLOCATION_COUNT)
        .map(|allocation| {
            let fill = cycle.wrapping_add(allocation as u64) as u8;
            vec![fill; BLOCKING_ALLOCATION_SIZE].into_boxed_slice()
        })
        .collect();
    let allocated_bytes = allocations.iter().map(|allocation| allocation.len()).sum();

    std::thread::sleep(BLOCKING_ALLOCATION_DELAY);
    std::hint::black_box(allocations);

    allocated_bytes
}

fn run_cache_workload(cycle: u64) -> usize {
    let tier = ObjectId::new(0x6172_7479_2d63_6163);
    let key = cycle % 4;
    let mut cache = std::collections::HashMap::new();

    record_cache(tier, EventKind::CacheMiss);
    let value = vec![key as u8; 4 * 1024];
    record_cache(tier, EventKind::CacheComputeSucceeded);
    cache.insert(key, value);
    record_cache(tier, EventKind::CacheInserted);

    let hits = usize::from(cache.get(&key).is_some());
    if hits != 0 {
        record_cache(tier, EventKind::CacheHit);
    }
    if cycle.is_multiple_of(4) && cache.remove(&key).is_some() {
        record_cache(tier, EventKind::CacheEvicted);
    }

    hits
}

fn record_cache(tier: ObjectId, kind: EventKind) {
    seismograph::record(EventClass::Cache, || Some(Record::object_measurement(kind, tier, 0)));
}

fn run_file_io(cycle: u64) -> usize {
    static RESOURCE: OnceLock<IoResourceId> = OnceLock::new();
    static BUFFER: OnceLock<BufferId> = OnceLock::new();

    let resource_id = *RESOURCE.get_or_init(IoResourceId::allocate);
    let buffer_id = *BUFFER.get_or_init(BufferId::allocate);
    let path = env::temp_dir().join(format!("arty-monitoring-{}.tmp", std::process::id()));
    let payload = vec![cycle as u8; IO_PAYLOAD_SIZE];
    let write_operation = IoOperationId::allocate();
    record_io(
        EventKind::IoWriteStarted,
        io_event(write_operation, resource_id, buffer_id, payload.len(), 0, IoOutcome::Pending),
    );
    let written = match std::fs::File::create(&path).and_then(|mut file| file.write_all(&payload)) {
        Ok(()) => payload.len(),
        Err(error) => {
            eprintln!("monitoring showcase file write failed: {error}");
            0
        }
    };
    record_io(
        EventKind::IoWriteFinished,
        io_event(
            write_operation,
            resource_id,
            buffer_id,
            payload.len(),
            written,
            if written == payload.len() {
                IoOutcome::Success
            } else {
                IoOutcome::Error
            },
        ),
    );

    let read_operation = IoOperationId::allocate();
    record_io(
        EventKind::IoReadStarted,
        io_event(read_operation, resource_id, buffer_id, payload.len(), 0, IoOutcome::Pending),
    );
    let read = match std::fs::read(&path) {
        Ok(bytes) => bytes.len(),
        Err(error) => {
            eprintln!("monitoring showcase file read failed: {error}");
            0
        }
    };
    record_io(
        EventKind::IoReadFinished,
        io_event(
            read_operation,
            resource_id,
            buffer_id,
            payload.len(),
            read,
            if read == payload.len() {
                IoOutcome::Success
            } else {
                IoOutcome::Error
            },
        ),
    );
    if let Err(error) = std::fs::remove_file(path) {
        eprintln!("monitoring showcase temporary-file cleanup failed: {error}");
    }

    written.saturating_add(read)
}

fn io_event(
    operation_id: IoOperationId,
    resource_id: IoResourceId,
    buffer_id: BufferId,
    requested_bytes: usize,
    completed_bytes: usize,
    outcome: IoOutcome,
) -> IoEvent {
    IoEvent {
        operation_id,
        resource_id,
        buffer_id: Some(buffer_id),
        requested_bytes: u64::try_from(requested_bytes).unwrap_or(u64::MAX),
        completed_bytes: u64::try_from(completed_bytes).unwrap_or(u64::MAX),
        buffer_len: u64::try_from(requested_bytes).unwrap_or(u64::MAX),
        buffer_span_count: 1,
        resource_kind: IoResourceKind::File,
        outcome,
    }
}

fn record_io(kind: EventKind, event: IoEvent) {
    seismograph::record(EventClass::Io, || Some(Record::io(kind, event)));
}
