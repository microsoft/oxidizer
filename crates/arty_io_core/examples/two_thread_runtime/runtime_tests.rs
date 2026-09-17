// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::{Command, Options, Runtime, WorkerLoop};
use crate::echo_driver::{EchoContext, EchoIoError};
use crate::sample_driver::{SampleContext, SampleIoError};
use crate::test_support::Harness;

#[test]
fn two_workers_lazily_cache_and_host_both_models_on_their_collectors() {
    let runtime = Runtime::start_with(Options {
        quantum: NonZeroUsize::new(1).unwrap(),
        ..Options::default()
    })
    .unwrap();
    let metrics: Vec<_> = runtime
        .workers
        .iter()
        .map(|worker| std::sync::Arc::clone(&worker.metrics))
        .collect();
    for metrics in &metrics {
        // Diagnostic reads; registration acknowledgements and operation results synchronize work.
        assert_eq!(metrics.records_created.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.readiness_created.load(Ordering::Relaxed), 0);
    }
    let sample = runtime.get_context::<SampleContext>().unwrap();
    assert_eq!(sample, runtime.get_context::<SampleContext>().unwrap());
    for metrics in &metrics {
        assert_eq!(metrics.records_created.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.readiness_created.load(Ordering::Relaxed), 0);
    }
    let echo = runtime.get_context::<EchoContext>().unwrap();
    assert_eq!(echo, runtime.get_context::<EchoContext>().unwrap());
    for (index, metrics) in metrics.iter().enumerate() {
        let sample = runtime.get_context_on::<SampleContext>(index).unwrap();
        let echo = runtime.get_context_on::<EchoContext>(index).unwrap();
        assert_eq!(sample.driver_thread(), metrics.owner);
        assert_eq!(echo.driver_thread(), metrics.owner);
        assert_ne!(metrics.owner, thread::current().id());
        let number = sample.submit(5).unwrap();
        let text = echo.submit("owned").unwrap();
        assert_eq!(number.wait().unwrap(), 6);
        assert_eq!(text.wait().unwrap(), "OWNED");
        assert_eq!(sample.completed_on(), Some(metrics.owner));
        assert_eq!(echo.completed_on(), Some(metrics.owner));
        assert_eq!(metrics.readiness_created.load(Ordering::Relaxed), 1);
    }
    assert_ne!(metrics[0].owner, metrics[1].owner);
    assert_ne!(
        runtime.get_context_on::<SampleContext>(0).unwrap(),
        runtime.get_context_on::<SampleContext>(1).unwrap()
    );
    runtime.shutdown().unwrap();
    for metrics in &metrics {
        assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 1);
    }
    assert!(matches!(sample.submit(5), Err(SampleIoError::Closed)));
    assert!(matches!(echo.submit("closed"), Err(EchoIoError::Closed)));
}

#[test]
fn later_worker_creation_failure_rolls_back_before_publishing_or_retrying() {
    let runtime = Runtime::start_with(Options {
        fail_record_worker: Some(1),
        ..Options::default()
    })
    .unwrap();
    let failure = runtime.get_context::<SampleContext>().unwrap_err();
    assert!(failure.to_string().contains("injected record registration failure"));
    assert!(runtime.contexts.lock().unwrap().is_empty());
    let first = std::sync::Arc::clone(&runtime.workers[0].metrics);
    let second = std::sync::Arc::clone(&runtime.workers[1].metrics);
    // Relaxed diagnostic reads follow the completed rollback acknowledgements.
    assert_eq!(first.records_created.load(Ordering::Relaxed), 1);
    assert_eq!(first.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(second.records_created.load(Ordering::Relaxed), 0);
    let context = runtime.get_context::<SampleContext>().unwrap();
    assert_eq!(context, runtime.get_context::<SampleContext>().unwrap());
    assert_eq!(first.records_created.load(Ordering::Relaxed), 2);
    assert_eq!(second.records_created.load(Ordering::Relaxed), 1);
    assert_eq!(context.submit(10).unwrap().wait().unwrap(), 11);
    let echo = runtime.get_context::<EchoContext>().unwrap();
    assert_eq!(echo.submit("independent").unwrap().wait().unwrap(), "INDEPENDENT");
    runtime.shutdown().unwrap();
    assert_eq!(first.records_retired.load(Ordering::Relaxed), 2);
    assert_eq!(second.records_retired.load(Ordering::Relaxed), 1);
}

#[test]
fn incompatible_actual_clients_roll_back_before_any_context_is_cached() {
    let runtime = Runtime::start_with(Options {
        missing_record_worker: Some(1),
        ..Options::default()
    })
    .unwrap();
    let failure = runtime.get_context::<SampleContext>().unwrap_err();
    assert!(failure.errors[0].is_unsupported());
    assert!(runtime.contexts.lock().unwrap().is_empty());
    let first = &runtime.workers[0].metrics;
    let second = &runtime.workers[1].metrics;
    assert_eq!(first.records_created.load(Ordering::Relaxed), 1);
    assert_eq!(first.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(second.records_created.load(Ordering::Relaxed), 0);
    let echo = runtime.get_context::<EchoContext>().unwrap();
    assert_eq!(echo.submit("compatible").unwrap().wait().unwrap(), "COMPATIBLE");
    runtime.shutdown().unwrap();
}

#[test]
fn one_overall_expired_deadline_abandons_all_drains_without_freeing_callbacks() {
    let mut harness = Harness::new(1);
    let sample = harness.install::<SampleContext>();
    let echo = harness.install::<EchoContext>();
    let number = sample.submit(2).unwrap();
    let text = echo.submit("pending").unwrap();
    let (_commands, receiver) = mpsc::channel();
    let mut worker = WorkerLoop {
        worker: harness.thread.clone(),
        tasks: harness.tasks.handle(),
        coordinator: harness.coordinator,
        commands: receiver,
        pending_command: None,
        rollbacks: std::collections::HashMap::new(),
        stopping: None,
        errors: Vec::new(),
        options: Options::default(),
    };
    worker.begin_stop(Instant::now());
    assert_eq!(harness.tasks.len(), 2);
    assert!(matches!(sample.submit(1), Err(SampleIoError::Closed)));
    assert!(matches!(echo.submit("late"), Err(EchoIoError::Closed)));
    let error = worker.run().unwrap_err();
    assert_eq!(error.errors.len(), 2);
    assert!(error.errors.iter().all(arty_io_core::DriverError::is_shutdown_timeout));
    assert!(matches!(number.wait(), Err(SampleIoError::Abandoned)));
    assert!(matches!(text.wait(), Err(EchoIoError::Abandoned)));
    harness.tasks.run_all();
}

#[test]
fn a_timed_out_owner_can_submit_cleanup_before_the_existing_pool_retires() {
    let mut runtime = Runtime::start().unwrap();
    let retained_context = runtime.get_context::<SampleContext>().unwrap();
    let retained_tasks = runtime.system.handle();
    let (original_tx, original_rx) = mpsc::channel();
    retained_tasks
        .spawn(move || original_tx.send(thread::current().id()).unwrap())
        .unwrap();
    let pool_thread = original_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (cleanup_tx, cleanup_rx) = mpsc::channel();
    let (accepted_tx, accepted_rx) = mpsc::channel();
    runtime.workers[0]
        .send(Command::Pause {
            entered: entered_tx,
            resume: resume_rx,
            cleanup: Box::new(move || cleanup_tx.send(thread::current().id()).unwrap()),
            accepted: accepted_tx,
        })
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.options.shutdown_timeout = Duration::ZERO;
    assert!(
        runtime
            .stop()
            .unwrap_err()
            .errors
            .iter()
            .any(arty_io_core::DriverError::is_shutdown_timeout)
    );
    assert!(matches!(runtime.workers[0].finished.try_recv(), Err(mpsc::TryRecvError::Empty)));

    resume_tx.send(()).unwrap();
    accepted_rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(cleanup_rx.recv_timeout(Duration::from_secs(5)).unwrap(), pool_thread);
    let owner_result = runtime.workers[0].finished.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        owner_result
            .unwrap_err()
            .errors
            .iter()
            .all(arty_io_core::DriverError::is_shutdown_timeout)
    );
    runtime.system.wait_stopped(Duration::from_secs(5)).unwrap();
    assert!(matches!(retained_context.submit(0), Err(SampleIoError::Closed)));
    // Neither an inert handle nor a closed consumer context participates in execution retirement.
    drop(retained_tasks);
    drop(retained_context);
}
