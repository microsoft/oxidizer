// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native condition registration is ordered before releasing its owned mutex.

#![cfg(feature = "seismograph")]

use std::sync::Arc;

use performables::sync::Sync;
use performables::sync::condition::Condvar;
use performables::sync::mutex::Mutex;
use seismograph::recorder::event::EventKind;

#[cfg(test)]
fn count(kind: EventKind) -> usize {
    let thread = seismograph::recorder::thread_id();
    seismograph::snapshot::decode(
        seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
            .unwrap()
            .as_bytes(),
    )
    .unwrap()
    .events
    .events
    .iter()
    .filter(|event| event.thread_id == thread && event.kind == kind)
    .count()
}

#[test]
fn gamma_survivor_native_condition_records_contention_before_release_and_access_after_notification() {
    seismograph::recorder(seismograph::recorder::Configuration {
        general_events: seismograph::recorder::Policy::all(false),
        event_capacity_per_thread: seismograph::recorder::EventBufferCapacity::new(128),
        ..Default::default()
    });
    let mutex = Arc::new(Mutex::<u32, Sync>::new(0));
    let condition = Arc::new(Condvar::<Sync>::new());
    let (ready, started) = std::sync::mpsc::channel();
    let worker_mutex = Arc::clone(&mutex);
    let worker_condition = Arc::clone(&condition);
    let worker = std::thread::spawn(move || {
        let before = (count(EventKind::CondvarContention), count(EventKind::CondvarAccess));
        let guard = worker_mutex.lock();
        ready.send(()).unwrap();
        let guard = worker_condition.wait_while(guard, |value| *value == 0);
        assert_eq!(*guard, 7);
        let waits = count(EventKind::CondvarContention) - before.0;
        assert!(waits >= 1);
        assert!(count(EventKind::CondvarAccess) - before.1 >= 1);
    });
    started.recv().unwrap();
    let mut guard = mutex.lock();
    *guard = 7;
    condition.notify_one();
    drop(guard);
    worker.join().unwrap();
    assert_eq!(*mutex.try_lock().unwrap(), 7);
}
