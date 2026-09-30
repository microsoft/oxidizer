// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime operations preserve processor snapshots and ownership.

#![cfg(feature = "rt")]
#![cfg(not(miri))] // Processor discovery and pinning require native hardware APIs.

testing_aids::init_tracing!();

use std::thread;

use arty::runtime::{ProcessorCount, Runtime, RuntimeOperations};
use many_cpus::{ProcessorId, SystemHardware};
use thread_aware::ThreadAware;

#[cfg(test)]
fn pinned_processor(operations: RuntimeOperations) -> ProcessorId {
    // Keep persistent affinity changes off the shared test-harness thread.
    thread::spawn(move || {
        operations.pin_current_thread();
        assert!(SystemHardware::current().is_thread_processor_pinned());
        SystemHardware::current().current_processor_id()
    })
    .join()
    .unwrap()
}

#[test]
fn runtime_operations_are_available_off_worker() {
    let runtime = Runtime::builder().processor_count(ProcessorCount::at_most(1)).build().unwrap();
    let (builtins, expected) = runtime
        .task_scheduler()
        .spawn(async |cx| (cx, SystemHardware::current().current_processor_id()))
        .wait()
        .unwrap();

    let actual = thread::spawn(move || {
        let operations = builtins.runtime_operations();
        let processor = pinned_processor(operations.clone());
        operations.stop();
        processor
    })
    .join()
    .unwrap();

    runtime.wait();
    assert_eq!(actual, expected);
}

#[test]
fn operations_and_builtins_follow_owner_relocation() {
    if SystemHardware::current().processors().len() < 2 {
        eprintln!("requires two processors to exercise cross-worker relocation");
        return;
    }
    let runtime = Runtime::builder().processor_count(ProcessorCount::exactly(2)).build().unwrap();
    let workers: Vec<_> = (0..2)
        .map(|_| {
            runtime
                .task_scheduler()
                .spawn(async |cx| (cx, SystemHardware::current().current_processor_id()))
        })
        .map(|handle| handle.wait().unwrap())
        .collect();
    let source = workers[0].0.thread().clone();
    let destination = workers[1].0.thread().clone();
    let mut builtins = workers[0].0.clone();
    let mut operations = builtins.runtime_operations().clone();
    let original_operations = operations.clone();

    operations.relocate(Some(&source), &destination);
    builtins.relocate(None, &destination);
    operations.relocate(Some(&destination), &destination);
    builtins.relocate(Some(&destination), &destination);

    assert_eq!(
        (
            pinned_processor(operations),
            pinned_processor(builtins.runtime_operations().clone()),
            pinned_processor(original_operations),
        ),
        (workers[1].1, workers[1].1, workers[0].1,),
    );
}

#[test]
fn operations_preserve_foreign_owner() {
    let source_runtime = Runtime::builder().processor_count(ProcessorCount::at_most(1)).build().unwrap();
    let other_runtime = Runtime::builder().processor_count(ProcessorCount::at_most(1)).build().unwrap();
    let (mut operations, source, processor) = source_runtime
        .task_scheduler()
        .spawn(async |cx| {
            (
                cx.runtime_operations().clone(),
                cx.thread().clone(),
                SystemHardware::current().current_processor_id(),
            )
        })
        .wait()
        .unwrap();
    let destination = other_runtime.task_scheduler().spawn(async |cx| cx.thread().clone()).wait().unwrap();

    operations.relocate(Some(&source), &destination);

    assert_eq!(pinned_processor(operations), processor);
}

#[test]
fn captured_processor_snapshot_pins_after_runtime_shutdown() {
    let (operations, processor) = Runtime::builder()
        .processor_count(ProcessorCount::at_most(1))
        .build()
        .unwrap()
        .run(async |cx| (cx.runtime_operations().clone(), SystemHardware::current().current_processor_id()))
        .unwrap();

    assert_eq!(
        (pinned_processor(operations.clone()), pinned_processor(operations)),
        (processor, processor),
    );
}

#[test]
fn maximum_processors_clamps_to_available_processors() {
    let available = SystemHardware::current().processors().len();
    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::at_most(usize::MAX))
        .build()
        .unwrap();
    let workers: std::collections::HashSet<_> = (0..available)
        .map(|_| runtime.task_scheduler().spawn(async |_| thread::current().id()))
        .map(|handle| handle.wait().unwrap())
        .collect();

    assert_eq!(workers.len(), available);
}
