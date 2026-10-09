// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-owned opaque panic payloads can panic when an abandoned join discards them.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

mod support;

use std::panic::panic_any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use panic_support::runtime;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;

testing_aids::init_tracing!();

struct Payload {
    drops: Arc<AtomicUsize>,
    dropped: mpsc::Sender<()>,
}

impl Drop for Payload {
    fn drop(&mut self) {
        assert!(!std::thread::panicking(), "the original task unwind was already caught");
        self.drops.fetch_add(1, Ordering::SeqCst);
        self.dropped.send(()).unwrap();
        panic!("discarded opaque panic payload");
    }
}

fn check(runtime: arty::runtime::Runtime, drops: &AtomicUsize, received: &mpsc::Receiver<()>) {
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn abandoned_remote_factory_payload_drop_is_contained() {
    let runtime = runtime();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let payload = Payload {
        drops: Arc::clone(&drops),
        dropped,
    };
    runtime
        .scheduler()
        .block_on(async move |cx| {
            let join = cx.scheduler().spawn(move |_| -> std::future::Ready<()> { panic_any(payload) });
            drop(join);
        })
        .unwrap();
    check(runtime, &drops, &received);
}

#[test]
fn abandoned_remote_poll_payload_drop_is_contained() {
    let runtime = runtime();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let payload = Payload {
        drops: Arc::clone(&drops),
        dropped,
    };
    runtime
        .scheduler()
        .block_on(async move |cx| {
            let join: arty::task::JoinHandle<()> = cx.scheduler().spawn(async move |_| panic_any(payload));
            drop(join);
        })
        .unwrap();
    check(runtime, &drops, &received);
}

#[test]
fn abandoned_blocking_payload_drop_is_contained() {
    let runtime = runtime();
    let (started, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let payload = Payload {
        drops: Arc::clone(&drops),
        dropped,
    };
    let join: arty::task::JoinHandle<()> = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        gate.recv_timeout(TEST_TIMEOUT).unwrap();
        panic_any(payload);
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    drop(join);
    release.send(()).unwrap();
    check(runtime, &drops, &received);
}
