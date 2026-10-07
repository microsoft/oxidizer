// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime operations select explicit worker affinity and preserve runtime ownership.

#![cfg(feature = "rt")]
#![cfg(not(miri))] // Processor discovery and pinning require native hardware APIs.

testing_aids::init_tracing!();

mod support;

use std::thread;

use arty::core::Thread;
use arty::runtime::{Runtime, RuntimeOperations, WorkersPolicy};
use many_cpus::{ProcessorId, SystemHardware};
use support::JoinHandleExt as _;
use thread_aware::{ThreadAware, ThreadBuilder, Unaware};

#[cfg(test)]
fn pinned_processor(operations: RuntimeOperations, worker: Thread) -> ProcessorId {
    // Keep persistent affinity changes off the shared test-harness thread.
    thread::spawn(move || {
        operations.pin_current_thread_to(&worker).unwrap();
        assert!(SystemHardware::current().is_thread_processor_pinned());
        SystemHardware::current().current_processor_id()
    })
    .join()
    .unwrap()
}

#[test]
fn operations_are_cloneable_but_not_thread_aware() {
    static_assertions::assert_impl_all!(RuntimeOperations: Clone, Send, Sync);
    static_assertions::assert_not_impl_any!(RuntimeOperations: ThreadAware);
}

#[test]
fn pinning_rejects_a_foreign_owner_with_a_registered_thread_id() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let worker = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.thread().clone() })
        .join()
        .unwrap();
    let foreign = ThreadBuilder::default().build(worker.id());
    let operations = RuntimeOperations::from(&runtime);
    assert!(
        thread::spawn(move || operations.pin_current_thread_to(&foreign))
            .join()
            .unwrap()
            .is_err()
    );
}

#[test]
fn runtime_operations_are_available_off_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::at_most(1)).build().unwrap();
    let (worker, builtins, expected) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move {
            (cx.thread().clone(), cx, SystemHardware::current().current_processor_id())
        })
        .join()
        .unwrap();

    let actual = thread::spawn(move || {
        let operations = RuntimeOperations::from(&builtins);
        let processor = pinned_processor(operations.clone(), worker);
        operations.request_stop();
        processor
    })
    .join()
    .unwrap();

    runtime.stop().unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn runtime_workers_cannot_change_their_processor_affinity() {
    let runtime = Runtime::builder().workers(WorkersPolicy::at_most(1)).build().unwrap();

    let async_error = runtime
        .scheduler()
        .block_on(async |cx| RuntimeOperations::from(&cx).pin_current_thread_to(cx.thread()))
        .unwrap()
        .unwrap_err();
    assert_eq!(async_error.to_string(), "an async Arty worker cannot change its processor affinity");

    let (operations, worker) = runtime
        .scheduler()
        .block_on(async |cx| (RuntimeOperations::from(&cx), cx.thread().clone()))
        .unwrap();
    let blocking_error = runtime
        .scheduler()
        .spawn_blocking(move || operations.pin_current_thread_to(&worker))
        .join()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        blocking_error.to_string(),
        "an Arty blocking callback cannot change its processor affinity"
    );

    runtime.stop().unwrap();
}

#[test]
fn explicit_targets_select_workers_without_relocating_operations() {
    if SystemHardware::current().processors().len() < 2 {
        eprintln!("requires two processors to exercise cross-worker relocation");
        return;
    }
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let workers: Vec<_> = (0..2)
        .map(|_| {
            runtime
                .scheduler()
                .spawn_anywhere((), |cx, ()| async move { (cx, SystemHardware::current().current_processor_id()) })
        })
        .map(|handle| handle.join().unwrap())
        .collect();
    let source = workers[0].0.thread().clone();
    let destination = workers[1].0.thread().clone();
    let mut builtins = workers[0].0.clone();
    let operations = RuntimeOperations::from(&builtins);
    let original_operations = operations.clone();

    builtins.relocate(None, &destination);
    builtins.relocate(Some(&destination), &destination);

    assert_eq!(
        (
            pinned_processor(operations, destination.clone()),
            pinned_processor(RuntimeOperations::from(&builtins), destination),
            pinned_processor(original_operations, source),
        ),
        (workers[1].1, workers[1].1, workers[0].1,),
    );
}

#[test]
fn operations_reject_foreign_workers_without_changing_runtime_identity() {
    let source_runtime = Runtime::builder().workers(WorkersPolicy::at_most(1)).build().unwrap();
    let other_runtime = Runtime::builder().workers(WorkersPolicy::at_most(1)).build().unwrap();
    let (Unaware(operations), source, processor) = source_runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move {
            (
                Unaware(RuntimeOperations::from(&cx)),
                cx.thread().clone(),
                SystemHardware::current().current_processor_id(),
            )
        })
        .join()
        .unwrap();
    let destination = other_runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.thread().clone() })
        .join()
        .unwrap();

    assert!(operations.pin_current_thread_to(&destination).is_err());

    assert_eq!(pinned_processor(operations, source), processor);
}

#[test]
fn captured_processor_snapshot_pins_after_runtime_shutdown() {
    let (operations, worker, processor) = Runtime::builder()
        .workers(WorkersPolicy::at_most(1))
        .build()
        .unwrap()
        .scheduler()
        .block_on(async |cx| {
            (
                RuntimeOperations::from(&cx),
                cx.thread().clone(),
                SystemHardware::current().current_processor_id(),
            )
        })
        .unwrap();

    assert_eq!(
        (
            pinned_processor(operations.clone(), worker.clone()),
            pinned_processor(operations, worker),
        ),
        (processor, processor),
    );
}

#[test]
fn runtime_and_builtin_conversions_use_the_same_affinity_information() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let operations = RuntimeOperations::from(&runtime);
    let (builtins, worker, processor) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move {
            let worker = cx.thread().clone();
            (cx, worker, SystemHardware::current().current_processor_id())
        })
        .join()
        .unwrap();
    assert_eq!(
        (
            pinned_processor(operations.clone(), worker.clone()),
            pinned_processor(RuntimeOperations::from(&builtins), worker),
        ),
        (processor, processor),
    );
    operations.request_stop();
    runtime.stop().unwrap();
}

#[test]
fn maximum_processors_clamps_to_available_processors() {
    let available = SystemHardware::current().processors().len();
    let runtime = Runtime::builder().workers(WorkersPolicy::at_most(usize::MAX)).build().unwrap();
    let workers: std::collections::HashSet<_> = (0..available)
        .map(|_| runtime.scheduler().spawn_anywhere((), |_, ()| async { thread::current().id() }))
        .map(|handle| handle.join().unwrap())
        .collect();

    assert_eq!(workers.len(), available);
}
