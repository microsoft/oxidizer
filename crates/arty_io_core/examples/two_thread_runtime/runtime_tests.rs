// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use std::{io, thread};

use arty_io_core::{DriverError, DriverProvider, IoContext, SystemTasks};
use thread_aware_core::ThreadAware;

use super::{Command, Options, Runtime, WorkerLoop};
use crate::echo_driver::{EchoContext, EchoIoError};
use crate::sample_driver::{SampleContext, SampleIoError};
use crate::test_support::{Harness, ManualTasks};

fn manual_worker(harness: Harness) -> (WorkerLoop, ManualTasks, mpsc::Sender<Command>) {
    let (sender, receiver) = mpsc::channel();
    let worker = WorkerLoop {
        worker: harness.thread,
        tasks: harness.tasks.handle(),
        coordinator: harness.coordinator,
        commands: receiver,
        pending_command: None,
        rollbacks: std::collections::HashMap::new(),
        completed_rollbacks: std::collections::HashMap::new(),
        stopping: None,
        errors: Vec::new(),
        options: Options::default(),
    };
    (worker, harness.tasks, sender)
}

fn sample_install(creations: Arc<AtomicUsize>, reply: mpsc::Sender<Result<super::ContextBox, DriverError>>) -> Command {
    Command::Install {
        id: TypeId::of::<SampleContext>(),
        install: Box::new(move |context| {
            creations.fetch_add(1, Ordering::Relaxed);
            let mut provider = SampleContext::provider()?;
            provider.relocate(None, context.thread());
            let (consumer, driver) = provider.create(context)?;
            Ok(super::Installed {
                context: Box::new(consumer),
                driver: Box::new(driver),
            })
        }),
        reply,
    }
}

#[test]
fn retry_during_rollback_is_rejected_before_creation_then_succeeds_after_retirement() {
    let mut harness = Harness::new(2);
    let old = harness.install::<SampleContext>();
    let metrics = harness.coordinator.waiter.metrics();
    let (mut worker, tasks, commands) = manual_worker(harness);
    let id = TypeId::of::<SampleContext>();
    // The controller's deadline has elapsed, but this owner has not processed retirement yet.
    let deadline = Instant::now();
    let (rollback_tx, rollback_rx) = mpsc::channel();
    worker.command(Command::Rollback {
        id,
        deadline,
        reply: rollback_tx,
    });
    assert!(matches!(old.submit(1), Err(SampleIoError::Closed)));
    assert_eq!(tasks.len(), 1);
    let creations = Arc::new(AtomicUsize::new(0));
    let (retry_tx, retry_rx) = mpsc::channel();
    commands.send(sample_install(Arc::clone(&creations), retry_tx)).unwrap();
    worker.process_commands();

    retry_rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err();
    assert_eq!(creations.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.records_created.load(Ordering::Relaxed), 1);
    assert_eq!(worker.rollbacks[&id].deadline, deadline);
    assert!(matches!(rollback_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

    worker.finish_drains();
    let retirement = rollback_rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err();
    assert!(retirement.errors()[0].is_shutdown_timeout());
    assert!(worker.coordinator.is_empty());
    tasks.run_all();
    let (fresh_tx, fresh_rx) = mpsc::channel();
    worker.command(sample_install(Arc::clone(&creations), fresh_tx));
    let fresh = *fresh_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap()
        .downcast::<SampleContext>()
        .unwrap();
    assert_eq!(creations.load(Ordering::Relaxed), 1);
    assert_ne!(fresh, old);
    let operation = fresh.submit(8).unwrap();
    worker.coordinator.service(Instant::now());
    assert_eq!(operation.wait().unwrap(), 9);
}

#[test]
fn lost_install_reply_retires_only_its_id_with_a_deadline_and_late_acknowledgement() {
    let mut harness = Harness::new(2);
    let healthy = harness.install::<EchoContext>();
    let metrics = harness.coordinator.waiter.metrics();
    let (mut worker, tasks, _commands) = manual_worker(harness);
    worker.options.shutdown_timeout = Duration::ZERO;
    let (reply, receiver) = mpsc::channel();
    drop(receiver);
    worker.command(sample_install(Arc::new(AtomicUsize::new(0)), reply));

    assert!(
        worker.stopping.is_none(),
        "losing one installation reply must not stop healthy registrations"
    );
    assert_eq!(tasks.len(), 1);
    assert!(worker.next_deadline().is_some_and(|deadline| deadline <= Instant::now()));
    let operation = healthy.submit("still healthy").unwrap();
    worker.finish_drains();
    assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 0);
    worker.coordinator.service(Instant::now());
    assert_eq!(operation.wait().unwrap(), "STILL HEALTHY");
    assert!(!worker.coordinator.has_failed());

    let (reply, receiver) = mpsc::channel();
    worker.command(Command::Rollback {
        id: TypeId::of::<SampleContext>(),
        deadline: Instant::now() + Duration::from_mins(1),
        reply,
    });
    let retirement = receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err();
    assert_eq!(retirement.errors().len(), 1);
    assert!(retirement.errors()[0].is_shutdown_timeout());
    assert!(worker.errors.iter().any(|error| error.to_string().contains("before publication")));
    tasks.run_all();
}

#[test]
fn repeated_rollback_preserves_the_original_deadline_and_reply() {
    let mut harness = Harness::new(2);
    harness.install::<SampleContext>();
    let (mut worker, tasks, _commands) = manual_worker(harness);
    let id = TypeId::of::<SampleContext>();
    let original = Instant::now() + Duration::from_mins(1);
    let (first_tx, first_rx) = mpsc::channel();
    worker.command(Command::Rollback {
        id,
        deadline: original,
        reply: first_tx,
    });
    for deadline in [original + Duration::from_mins(1), Instant::now()] {
        let (reply, receiver) = mpsc::channel();
        worker.command(Command::Rollback { id, deadline, reply });
        let error = receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err();
        assert_eq!(error.errors()[0].to_string(), "registration rollback is already in progress");
        assert_eq!(worker.rollbacks[&id].deadline, original);
        assert!(matches!(first_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(tasks.len(), 1);
    }
    tasks.run_all();
    worker.coordinator.service(Instant::now());
    worker.finish_drains();
    first_rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    let (reply, receiver) = mpsc::channel();
    worker.command(Command::Rollback {
        id,
        deadline: original,
        reply,
    });
    let error = receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err();
    assert_eq!(error.errors()[0].to_string(), "registration rollback outcome was already delivered");
}

#[test]
fn controller_joins_orphan_retirement_without_extending_its_bound() {
    let (mut worker, tasks, _commands) = manual_worker(Harness::new(2));
    worker.options.shutdown_timeout = Duration::from_mins(1);
    let id = TypeId::of::<SampleContext>();
    let (reply, receiver) = mpsc::channel();
    drop(receiver);
    worker.command(sample_install(Arc::new(AtomicUsize::new(0)), reply));
    let original = worker.rollbacks[&id].deadline;
    let (reply, receiver) = mpsc::channel();
    worker.command(Command::Rollback {
        id,
        deadline: original + Duration::from_mins(1),
        reply,
    });
    assert_eq!(worker.rollbacks[&id].deadline, original);
    assert_eq!(tasks.len(), 1);
    assert!(matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)));
    tasks.run_all();
    worker.coordinator.service(Instant::now());
    worker.finish_drains();
    receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    assert!(worker.stopping.is_none());
    assert!(worker.coordinator.is_empty());
}

#[test]
fn unobserved_retirement_failure_reaches_worker_outcome_with_native_cause() {
    let (mut worker, _tasks, _commands) = manual_worker(Harness::new(2));
    worker.tasks = SystemTasks::new(|task| {
        drop(task);
        Err(DriverError::unsupported("cleanup execution unavailable")
            .with_cause(io::Error::new(io::ErrorKind::PermissionDenied, "native cleanup admission denied")))
    });
    let (reply, receiver) = mpsc::channel();
    worker.command(sample_install(Arc::new(AtomicUsize::new(0)), reply));
    let _context = receiver.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    let (reply, receiver) = mpsc::channel();
    drop(receiver);
    worker.command(Command::Rollback {
        id: TypeId::of::<SampleContext>(),
        deadline: Instant::now() + Duration::from_mins(1),
        reply,
    });
    let error = worker.run().unwrap_err();
    assert_eq!(error.errors().len(), 1);
    let failure = &error.errors()[0];
    assert!(failure.is_unsupported());
    assert_eq!(failure.to_string(), "cleanup execution unavailable");
    let native = failure.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(native.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(native.to_string(), "native cleanup admission denied");
}

#[test]
fn fresh_retry_keeps_an_unclaimed_retirement_failure_observable() {
    let (mut worker, tasks, _commands) = manual_worker(Harness::new(2));
    worker.options.shutdown_timeout = Duration::ZERO;
    let (reply, receiver) = mpsc::channel();
    drop(receiver);
    worker.command(sample_install(Arc::new(AtomicUsize::new(0)), reply));
    worker.finish_drains();
    assert!(worker.coordinator.is_empty());

    let (reply, receiver) = mpsc::channel();
    worker.command(sample_install(Arc::new(AtomicUsize::new(0)), reply));
    let context = *receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap()
        .downcast::<SampleContext>()
        .unwrap();
    let operation = context.submit(4).unwrap();
    worker.coordinator.service(Instant::now());
    assert_eq!(operation.wait().unwrap(), 5);
    assert!(worker.errors.iter().any(DriverError::is_shutdown_timeout));
    worker.begin_stop(Instant::now() + Duration::from_mins(1));
    tasks.run_all();
    worker.coordinator.service(Instant::now());
    worker.finish_drains();
    let error = worker.run().unwrap_err();
    assert_eq!(error.errors().len(), 2);
    assert!(error.errors().iter().any(DriverError::is_shutdown_timeout));
}

#[test]
fn aggregate_failures_preserve_every_typed_cause_in_order() {
    let error = super::outcome(vec![
        DriverError::unsupported("native strategy unavailable")
            .with_cause(io::Error::new(io::ErrorKind::PermissionDenied, "native binding denied")),
        DriverError::shutdown_timeout().with_cause(io::Error::new(io::ErrorKind::TimedOut, "native cleanup timed out")),
    ])
    .unwrap_err();
    let failures = error.errors();
    assert_eq!(failures.len(), 2);
    assert!(failures[0].is_unsupported());
    assert!(!failures[0].is_shutdown_timeout());
    assert!(failures[1].is_shutdown_timeout());
    assert!(!failures[1].is_unsupported());
    let native_causes: Vec<_> = failures
        .iter()
        .map(|failure| failure.source().unwrap().downcast_ref::<io::Error>().unwrap().kind())
        .collect();
    assert_eq!(native_causes, [io::ErrorKind::PermissionDenied, io::ErrorKind::TimedOut]);
    let primary = error.source().unwrap().downcast_ref::<DriverError>().unwrap();
    assert!(std::ptr::eq(primary, &raw const failures[0]));
}

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
        completed_rollbacks: std::collections::HashMap::new(),
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
            cleanup: arty_io_core::SystemTask::new(move || cleanup_tx.send(thread::current().id()).unwrap()),
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
