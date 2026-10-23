// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ready fast paths and blocking fallbacks retain their public telemetry.

#![expect(clippy::unwrap_used, reason = "test helpers surface setup and snapshot failures")]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use performables::sync as mode;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{bounded, oneshot, watch};
use performables::sync::condition::Condvar;
use seismograph::recorder::event::{Event, EventKind, ObjectId};
use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
use seismograph::snapshot::{SnapshotOptions, decode};
use testing_aids::is_mutation_testing;

const DEADLINE: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 5 });

fn events() -> Vec<Event> {
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    decode(encoded.as_bytes()).unwrap().events.events
}

fn ready_events() {
    let thread_id = seismograph::recorder::current_thread_id();
    let native = Box::new(Barrier::<mode::Sync>::new(1));
    let asynchronous = Box::new(Barrier::<mode::Async>::new(1));
    let condition = Box::new(Condvar::<mode::Sync>::new());
    let ids = [
        ObjectId::from_ptr(std::ptr::from_ref(&*native).cast::<()>()),
        ObjectId::from_ptr(std::ptr::from_ref(&*asynchronous).cast::<()>()),
        ObjectId::from_ptr(std::ptr::from_ref(&*condition).cast::<()>()),
    ];
    for _ in 0..3 {
        assert!(native.wait().is_leader());
        assert!(asynchronous.wait().is_leader());
    }
    condition.notify_one();
    condition.notify_all();

    let (sender, receiver) = bounded(1);
    sender.send(7).unwrap();
    assert_eq!(receiver.recv(), Ok(7));
    assert!(receiver.recv_timeout(Duration::ZERO).unwrap_err().is_timeout());
    let (sender, receiver) = oneshot();
    sender.send(11).unwrap();
    assert_eq!(receiver.recv(), Ok(11));
    let (_sender, receiver) = oneshot::<u64>();
    assert!(receiver.recv_timeout(Duration::ZERO).unwrap_err().is_timeout());
    let (sender, receiver) = watch(0);
    sender.send(17).unwrap();
    assert_eq!(receiver.changed(), Ok(()));

    let captured = events();
    for id in &ids[..2] {
        let observed = captured
            .iter()
            .filter(|event| event.object_id() == Some(*id))
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert_eq!(observed, [EventKind::BarrierAccess, EventKind::BarrierRelease].repeat(3));
    }
    assert_eq!(
        captured
            .iter()
            .filter(|event| event.object_id() == Some(ids[2]))
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        [EventKind::CondvarNotify; 2],
    );
    let count = |kind| {
        captured
            .iter()
            .filter(|event| event.thread_id == thread_id && event.kind == kind)
            .count()
    };
    assert_eq!(
        (
            count(EventKind::ChannelSend),
            count(EventKind::ChannelReceive),
            count(EventKind::ChannelSendContention),
            count(EventKind::ChannelReceiveContention),
        ),
        (3, 3, 0, 2),
    );
}

fn blocking_send_contention() {
    let (sender, receiver) = bounded(1);
    sender.send(7).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(seismograph::recorder::current_thread_id()).unwrap();
        finished_tx.send(sender.send(11)).unwrap();
    });
    let thread_id = if is_mutation_testing() {
        started_rx.recv().unwrap()
    } else {
        started_rx.recv_timeout(DEADLINE).unwrap()
    };
    let deadline = (!is_mutation_testing()).then(|| Instant::now() + DEADLINE);
    loop {
        assert!(!worker.is_finished(), "the full queue must block its sender");
        if events()
            .iter()
            .any(|event| event.thread_id == thread_id && event.kind == EventKind::ChannelSendContention)
        {
            break;
        }
        if let Some(deadline) = deadline {
            assert!(Instant::now() < deadline, "the blocked sender must record contention");
        }
        std::thread::yield_now();
    }
    assert_eq!(receiver.recv(), Ok(7));
    let result = if is_mutation_testing() {
        finished_rx.recv().unwrap()
    } else {
        finished_rx.recv_timeout(DEADLINE).unwrap()
    };
    assert_eq!(result, Ok(()));
    worker.join().unwrap();
    assert_eq!(receiver.recv(), Ok(11));
    assert_eq!(
        events()
            .iter()
            .filter(|event| event.thread_id == thread_id && event.kind == EventKind::ChannelSendContention)
            .count(),
        1,
    );
}

#[test]
fn ready_operations_and_blocking_handoffs_preserve_events() {
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(256).unwrap(),
        ..Default::default()
    });
    ready_events();
    blocking_send_contention();
    seismograph::recorder(Configuration::default());
}

testing_aids::init_tracing!();
