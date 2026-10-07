// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Abandoned joins dispose of original task panics without resuming them.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::cell::Cell;
use std::panic::panic_any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use panic_support::runtime;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct Payload {
    value: Cell<u32>,
    drops: Arc<AtomicUsize>,
    dropped: mpsc::Sender<()>,
}

impl Drop for Payload {
    fn drop(&mut self) {
        assert_eq!(self.value.get(), 42);
        assert!(!std::thread::panicking());
        self.drops.fetch_add(1, Ordering::SeqCst);
        self.dropped.send(()).unwrap();
    }
}

#[test]
fn abandoned_remote_factory_and_poll_panics_dispose_opaque_payloads_once() {
    for factory_panic in [true, false] {
        let runtime = runtime();
        let (dropped, received) = mpsc::channel();
        let (release, gate) = events_once::Event::boxed();
        let drops = Arc::new(AtomicUsize::new(0));
        let data = Unaware((factory_panic, gate, Arc::clone(&drops), dropped));
        let task: arty::task::JoinHandle<()> =
            runtime
                .scheduler()
                .spawn_anywhere(data, |_, Unaware((factory_panic, gate, drops, dropped))| {
                    let payload = Payload {
                        value: Cell::new(42),
                        drops,
                        dropped,
                    };
                    if factory_panic {
                        panic_any(payload);
                    }
                    async move {
                        gate.await.unwrap();
                        panic_any(payload);
                    }
                });
        drop(task);
        if !factory_panic {
            release.send(());
        }
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).wait().unwrap(), 7);
        runtime.stop().unwrap();
    }
}

#[test]
fn abandoned_local_panic_disposes_opaque_payload_on_its_worker() {
    let runtime = runtime();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&drops);
    runtime
        .scheduler()
        .block_on(async move |cx| {
            let (release, gate) = events_once::Event::boxed();
            let (started, ready) = events_once::Event::boxed();
            let task: arty::task::LocalJoinHandle<()> = cx.local_scheduler().unwrap().spawn(async move || {
                let payload = Payload {
                    value: Cell::new(42),
                    drops: captured,
                    dropped,
                };
                started.send(());
                gate.await.unwrap();
                panic_any(payload);
            });
            drop(task);
            ready.await.unwrap();
            release.send(());
            cx.scheduler().spawn(async |_| {}).await.unwrap();
        })
        .unwrap();
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}
