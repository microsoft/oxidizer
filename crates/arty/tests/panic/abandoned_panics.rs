// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Abandoned joins dispose of original task panics without resuming them.

#![cfg(feature = "rt")]
#![cfg(test)]

use std::cell::Cell;
use std::panic::panic_any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

use crate::runtime_support::runtime;
use crate::support::JoinHandleExt as _;

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
        assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).join().unwrap(), 7);
        runtime.stop().unwrap();
    }
}
