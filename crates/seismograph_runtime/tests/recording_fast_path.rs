// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stable call sites for disabled-hook regression checks and optimized assembly
//! inspection: `cargo rustc -p seismograph_runtime --release --test
//! recording_fast_path -- --emit=asm`.

use seismograph::recorder::runtime::TypeDescriptorId;
use seismograph_runtime::task::{TaskHandle, TaskPoll};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, register_runtime};

// Keep measurement entry points visible in optimized assembly. These annotations
// are test instrumentation, not changes to production inlining policy.
#[inline(never)]
fn wake_probe(task: &TaskHandle) {
    task.woken();
}

#[inline(never)]
fn poll_start_probe(task: &TaskHandle, worker: &WorkerHandle) -> TaskPoll {
    task.poll_started(worker)
}

#[inline(never)]
fn poll_finish_probe(task: &TaskHandle, worker: &WorkerHandle, poll: TaskPoll) {
    task.poll_finished(worker, poll);
}

#[test]
fn disabled_hooks_preserve_counters_without_creating_event_buffers() {
    seismograph::recorder(seismograph::recorder::Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("assembly-probe", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core)).handle();
    let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
    wake_probe(std::hint::black_box(&task));
    let poll = poll_start_probe(std::hint::black_box(&task), std::hint::black_box(&worker));
    poll_finish_probe(std::hint::black_box(&task), std::hint::black_box(&worker), poll);
    let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    assert_eq!(runtime.counters().snapshot().poll_count, 1);
    assert!(snapshot.events.events.is_empty());
    assert!(snapshot.events.threads.is_empty());
}
