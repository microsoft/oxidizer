// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(
    not(all(feature = "rt", feature = "seismograph")),
    expect(missing_docs, reason = "the feature-disabled integration target is empty")
)]
#![cfg(all(feature = "rt", feature = "seismograph"))]

//! Public live-task inspection, bounded callback delivery, and observation-only timing.

testing_aids::init_tracing!();

mod support;

use std::net::TcpStream;
use std::num::NonZeroUsize;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use arty::runtime::{Runtime, WorkersPolicy};
use events_once::Event;
use seismograph::recorder::Configuration;
use seismograph::recorder::runtime::RuntimeId;
use seismograph::snapshot::SnapshotOptions;
use seismograph_runtime::live::{Measurements, QueryOptions, SubscriptionOptions, TaskBatch, TaskCollection, subscribe};
use seismograph_runtime::snapshot::{TaskActivityState, TaskMetricsScope, TaskSource, source};
use support::JoinHandleExt;
use thread_aware::Unaware;

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[expect(clippy::unwrap_used, reason = "this helper only constructs test runtimes")]
fn runtime() -> Runtime {
    Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap()
}

#[expect(clippy::unwrap_used, reason = "this helper only converts validated test identities")]
fn id(runtime: &Runtime) -> RuntimeId {
    RuntimeId::from_raw(runtime.id().get()).unwrap()
}

#[expect(clippy::unwrap_used, reason = "this helper asserts bounded test callback delivery")]
fn receive(receiver: &mpsc::Receiver<TaskBatch>, predicate: impl Fn(&TaskBatch) -> bool) -> TaskBatch {
    let deadline = Instant::now() + testing_aids::TEST_TIMEOUT;
    loop {
        let batch = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
        if predicate(&batch) {
            return batch;
        }
    }
}

#[test]
fn live_tasks_are_visible_without_recording_or_historical_timing() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (release, wait) = Event::<()>::boxed();
    let (started, ready) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((wait, started)), |_, Unaware((wait, started))| async move {
            started.send(()).unwrap();
            wait.await.unwrap();
        });
    ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let (sender, receiver) = mpsc::channel();
    let subscription = subscribe(id(&runtime), SubscriptionOptions::default(), move |batch| {
        let _ = sender.send(batch);
    })
    .unwrap();
    let batch = receive(&receiver, |batch| {
        batch.collection == TaskCollection::Complete && batch.tasks.len() == 1
    });
    let task_id = batch.tasks[0].id;
    assert_eq!(
        (
            batch.tasks[0].metrics_scope,
            batch.tasks[0].spawned_at.ticks(),
            batch.tasks[0].activity.unwrap().state
        ),
        (TaskMetricsScope::Uncollected, 0, TaskActivityState::Waiting)
    );

    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let payload = decoded.sources.iter().find(|source_data| source_data.id == source::ID).unwrap();
    let source_snapshot = seismograph_runtime::snapshot::decode(&payload.data).unwrap();
    let observed = source_snapshot.runtimes.iter().find(|entry| entry.id == id(&runtime)).unwrap();
    assert_eq!(observed.task_source, TaskSource::Live);
    assert!(
        decoded
            .events
            .events
            .iter()
            .all(|event| { event.runtime().is_none_or(|payload| payload.runtime_id != id(&runtime)) })
    );

    release.send(());
    task.join().unwrap();
    let batch = receive(&receiver, |batch| {
        batch.collection == TaskCollection::Complete && batch.tasks.iter().all(|task| task.id != task_id)
    });
    assert!(batch.tasks.is_empty());
    drop(subscription);
    runtime.stop().unwrap();
}

#[test]
fn the_seismograph_monitor_protocol_can_capture_live_tasks_without_recording() {
    use seismograph_protocol::message::{Request, Response, SnapshotOptions as RemoteSnapshotOptions};
    use seismograph_protocol::{read_response, write_request};

    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let monitor = seismograph::monitor::Monitor::builder()
        .name("arty-live-protocol-test")
        .start()
        .unwrap();
    let (release, waiting) = Event::<()>::boxed();
    let (started, ready) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((waiting, started)), |_, Unaware((waiting, started))| async move {
            started.send(()).unwrap();
            waiting.await.unwrap();
        });
    ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let mut connection = TcpStream::connect_timeout(&monitor.descriptor().socket_address().into(), testing_aids::TEST_TIMEOUT).unwrap();
    connection.set_read_timeout(Some(testing_aids::TEST_TIMEOUT)).unwrap();
    connection.set_write_timeout(Some(testing_aids::TEST_TIMEOUT)).unwrap();
    write_request(
        &mut connection,
        1,
        &Request::Hello {
            authentication: monitor.descriptor().authentication,
        },
    )
    .unwrap();
    let (response_id, response) = read_response(&mut connection).unwrap();
    assert_eq!(response_id, 1);
    let Response::Hello { recording, .. } = response else {
        panic!("the monitor accepts the authenticated protocol handshake");
    };
    assert!(!recording.runtime_tasks.enabled);
    write_request(&mut connection, 2, &Request::CaptureSnapshot(RemoteSnapshotOptions::default())).unwrap();
    let (response_id, response) = read_response(&mut connection).unwrap();
    assert_eq!(response_id, 2);
    let Response::Snapshot(bytes) = response else {
        panic!("the monitor returns an encoded snapshot");
    };
    let decoded = seismograph::snapshot::decode(&bytes).unwrap();
    let payload = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let source_snapshot = seismograph_runtime::snapshot::decode(&payload.data).unwrap();
    let observed = source_snapshot.runtimes.iter().find(|entry| entry.id == id(&runtime)).unwrap();
    assert_eq!(
        (observed.task_source, observed.workers[0].task_collection, observed.tasks.len()),
        (TaskSource::Live, TaskCollection::Complete, 1)
    );
    assert_eq!(observed.tasks[0].metrics_scope, TaskMetricsScope::Uncollected);
    release.send(());
    task.join().unwrap();
    runtime.stop().unwrap();
}

#[test]
fn task_requests_are_bounded_and_report_truncation() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (started, ready) = mpsc::channel();
    let tasks = (0..3)
        .map(|_| {
            runtime
                .scheduler()
                .spawn_anywhere(Unaware(started.clone()), |_, Unaware(started)| async move {
                    started.send(()).unwrap();
                    std::future::pending::<()>().await;
                })
        })
        .collect::<Vec<_>>();
    for _ in 0..3 {
        ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    }
    let (sender, receiver) = mpsc::channel();
    let subscription = subscribe(
        id(&runtime),
        SubscriptionOptions {
            query: QueryOptions {
                limit_per_worker: NonZeroUsize::new(1).unwrap(),
                ..QueryOptions::default()
            },
            ..SubscriptionOptions::default()
        },
        move |batch| {
            let _ = sender.send(batch);
        },
    )
    .unwrap();
    let batch = receive(&receiver, |batch| batch.collection == TaskCollection::Truncated);
    assert_eq!(batch.tasks.len(), 1);
    drop(subscription);
    runtime.stop().unwrap();
    for task in tasks {
        assert!(task.join().unwrap_err().is_shutdown());
    }
}

#[test]
fn a_worker_blocked_before_monitoring_does_not_invent_current_task_history() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (started, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, blocked)), |_, Unaware((started, blocked))| async move {
            started.send(()).unwrap();
            blocked.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
        });
    ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let payload = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let source_snapshot = seismograph_runtime::snapshot::decode(&payload.data).unwrap();
    let observed = source_snapshot.runtimes.iter().find(|entry| entry.id == id(&runtime)).unwrap();
    let actual = (
        observed.workers[0].task_collection,
        observed.workers[0].current_task.is_some(),
        observed.tasks.is_empty(),
    );
    release.send(()).unwrap();
    task.join().unwrap();
    runtime.stop().unwrap();
    assert_eq!(actual, (TaskCollection::TimedOut, false, true));
}

#[test]
fn an_already_monitored_worker_exposes_its_current_task_if_it_stalls() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (sender, receiver) = mpsc::channel();
    let subscription = subscribe(id(&runtime), SubscriptionOptions::default(), move |batch| {
        let _ = sender.send(batch);
    })
    .unwrap();
    receive(&receiver, |batch| batch.collection == TaskCollection::Complete);
    let (started, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, blocked)), |_, Unaware((started, blocked))| async move {
            started.send(()).unwrap();
            blocked.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
        });
    ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let payload = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let source_snapshot = seismograph_runtime::snapshot::decode(&payload.data).unwrap();
    let worker = &source_snapshot
        .runtimes
        .iter()
        .find(|entry| entry.id == id(&runtime))
        .unwrap()
        .workers[0];
    let unavailable = matches!(worker.task_collection, TaskCollection::TimedOut | TaskCollection::Busy);
    let identified = worker.current_task.is_some();
    release.send(()).unwrap();
    task.join().unwrap();
    drop(subscription);
    runtime.stop().unwrap();
    assert_eq!((unavailable, identified), (true, true));
}

#[test]
fn unmonitored_execution_does_not_accumulate_task_or_poll_counters() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    for _ in 0..20 {
        runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap();
    }
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let payload = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let source_snapshot = seismograph_runtime::snapshot::decode(&payload.data).unwrap();
    let observed = source_snapshot.runtimes.iter().find(|entry| entry.id == id(&runtime)).unwrap();
    assert_eq!(observed.counters, seismograph_runtime::snapshot::Counters::default());
    assert_eq!(observed.counters_scope, seismograph_runtime::snapshot::CountersScope::Observed);
    runtime.stop().unwrap();
}

#[test]
fn ending_recording_stops_bookkeeping_for_tasks_that_are_still_alive() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: seismograph::recorder::RecordingPolicy::all(false),
        event_capacity_per_thread: seismograph::recorder::EventBufferCapacity::new(256).unwrap(),
        ..Configuration::default()
    });
    let runtime = runtime();
    let (wake, waiting) = performables::sync::channel::unbounded::<()>();
    let (started, ready) = mpsc::channel();
    let (ack, acknowledged) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((waiting, started, ack)), |_, Unaware((waiting, started, ack))| async move {
            started.send(()).unwrap();
            while waiting.recv_async().await.is_ok() {
                ack.send(()).unwrap();
            }
        });
    ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let before = read_counters(&runtime);
    assert!(before.poll_count > 0);
    seismograph::recorder(Configuration::default());
    for _ in 0..2 {
        wake.send(()).unwrap();
        acknowledged.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
        assert_eq!(read_counters(&runtime), before);
    }
    drop(wake);
    task.join().unwrap();
    assert_eq!(read_counters(&runtime), before);
    runtime.stop().unwrap();
}

#[test]
fn scoped_future_size_survives_erasure_without_recording_or_eager_descriptors() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (release, waiting) = Event::<()>::boxed();
    let (started, ready) = mpsc::channel();
    let (sender, receiver) = mpsc::channel();
    let subscription = subscribe(id(&runtime), SubscriptionOptions::default(), move |batch| {
        let _ = sender.send(batch);
    })
    .unwrap();
    let padding = [0_u8; 513];
    let future = async {
        started.send(()).unwrap();
        waiting.await.unwrap();
        std::hint::black_box(&padding);
    };
    let expected_size = size_of_val(&future) as u64;
    std::thread::scope(|scope| {
        let observation = scope.spawn(move || {
            ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
            let batch = receive(&receiver, |batch| {
                batch.collection == TaskCollection::Complete && batch.tasks.len() == 1
            });
            let size = batch.tasks[0].future_size_bytes;
            release.send(());
            size
        });
        runtime.scheduler().block_on(move |_| future).unwrap();
        assert_eq!(observation.join().unwrap(), Some(expected_size));
    });
    drop(subscription);
    runtime.stop().unwrap();
}

#[expect(clippy::unwrap_used, reason = "this helper reads validated test snapshots")]
fn read_counters(runtime: &Runtime) -> seismograph_runtime::snapshot::Counters {
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let payload = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let source_snapshot = seismograph_runtime::snapshot::decode(&payload.data).unwrap();
    source_snapshot
        .runtimes
        .iter()
        .find(|entry| entry.id == id(runtime))
        .unwrap()
        .counters
}

#[test]
fn slow_callbacks_do_not_block_task_execution_or_cancellation() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (entered, entry) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let subscription = subscribe(id(&runtime), SubscriptionOptions::default(), move |_| {
        entered.send(()).unwrap();
        blocked.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    })
    .unwrap();
    entry.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let (finished, completion) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware(finished), |_, Unaware(finished)| async move {
            finished.send(42).unwrap();
        });
    let actual = completion.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    drop(subscription);
    release.send(()).unwrap();
    task.join().unwrap();
    runtime.stop().unwrap();
    assert_eq!(actual, 42);
}

#[test]
fn timing_starts_on_demand_and_stops_after_the_last_interested_subscription() {
    let _lock = TEST_LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = runtime();
    let (wake, waiting) = performables::sync::channel::unbounded::<()>();
    let (started, ready) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((waiting, started)), |_, Unaware((waiting, started))| async move {
            started.send(()).unwrap();
            while waiting.recv_async().await.is_ok() {}
        });
    ready.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let (sender, receiver) = mpsc::channel();
    let subscription = subscribe(
        id(&runtime),
        SubscriptionOptions {
            measurements: Measurements::PollTiming,
            refresh_interval: Duration::from_millis(50),
            ..SubscriptionOptions::default()
        },
        move |batch| {
            let _ = sender.send(batch);
        },
    )
    .unwrap();
    wake.send(()).unwrap();
    let measured = receive(&receiver, |batch| {
        batch
            .tasks
            .iter()
            .any(|task| matches!(task.metrics_scope, TaskMetricsScope::PollsSince(_)) && task.metrics.poll_count > 0)
    });
    assert!(measured.tasks[0].metrics.poll_duration_nanos > 0);
    drop(subscription);
    wake.send(()).unwrap();
    let (sender, receiver) = mpsc::channel();
    let state_only = subscribe(id(&runtime), SubscriptionOptions::default(), move |batch| {
        let _ = sender.send(batch);
    })
    .unwrap();
    let unmeasured = receive(&receiver, |batch| batch.tasks.len() == 1);
    assert_eq!(unmeasured.tasks[0].metrics_scope, TaskMetricsScope::Uncollected);
    drop(state_only);
    drop(wake);
    task.join().unwrap();
    runtime.stop().unwrap();
}
