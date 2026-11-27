// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Checks process and component snapshots at known points in the worker queue.

use std::pin::pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use futures::executor::block_on;
use health::{Aggregator, Component, Health};
use observed::Sink;
use testing_aids::is_mutation_testing;

const TURN_TIMEOUT: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 3 });

struct Notify(mpsc::Sender<Thread>);

impl Wake for Notify {
    fn wake(self: Arc<Self>) {
        self.0
            .send(thread::current())
            .expect("probe receiver lives until the turn completes");
    }
}

// The probe is queued after the operation under test. Its notification acknowledges a worker
// turn even if the operation failed to change the state we intend to assert.
fn turn<T>(probe: &mut Component, publish: impl FnOnce() -> T) -> (T, Thread) {
    let (send, ack) = mpsc::channel();
    let waker = Waker::from(Arc::new(Notify(send)));
    let mut cx = Context::from_waker(&waker);
    let mut changed = pin!(probe.changed());
    assert!(changed.as_mut().poll(&mut cx).is_pending());
    let result = publish();
    let worker = if is_mutation_testing() {
        ack.recv().expect("worker must acknowledge the probe")
    } else {
        ack.recv_timeout(TURN_TIMEOUT).expect("worker must acknowledge the probe")
    };
    assert!(changed.as_mut().poll(&mut cx).is_ready());
    (result, worker)
}

#[test]
fn process_and_component_snapshots_follow_registration_publication_and_removal() {
    let root = Aggregator::new(Sink::noop());
    let reader = root.monitor();
    let mut probe = root.component("probe");
    let producer = probe.clone();
    let (mut marker, worker) = turn(&mut probe, || producer.publisher());
    assert!(
        worker.name().is_some_and(|name| name.to_ascii_lowercase().contains("health")),
        "thread diagnostics must identify the health subsystem"
    );

    let mut revision = 0_i64;
    let mut flush = || {
        revision += 1;
        let ((), observed) = turn(&mut probe, || marker.publish(Health::Healthy, [("turn", revision.to_string())]));
        assert_eq!(observed.id(), worker.id());
    };
    let mut clients = root.component("clients");
    flush();
    assert_eq!(reader.state(), Health::Unhealthy, "a newly registered empty component is unhealthy");
    assert_eq!(block_on(reader.reports()).count(), 2);

    let writer = clients.clone();
    let (mut publisher, observed) = turn(&mut clients, || writer.publisher());
    assert_eq!(observed.id(), worker.id(), "both components must run on the same worker");
    publisher.publish(Health::Degraded, [("load", "7")]);
    let clone = publisher.clone();
    flush();
    assert_eq!(clients.state(), Health::Degraded);
    assert_eq!(reader.state(), Health::Degraded);
    let report = block_on(clients.report());
    assert_eq!(report.publisher_count(Health::Healthy), 1, "the clone contributes independently");
    drop(publisher);
    flush();
    assert_eq!(reader.state(), Health::Healthy);
    drop(clone);
    flush();
    assert_eq!(reader.state(), Health::Unhealthy);
    drop((clients, writer));
    flush();
    assert_eq!(reader.state(), Health::Healthy, "removal leaves just the healthy probe");
}
