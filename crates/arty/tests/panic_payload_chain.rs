// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Secondary opaque panic payloads must not escape abandoned-result containment.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::panic::panic_any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use panic_support::{isolated, runtime};
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct DropChain {
    depth: u32,
    result_drops: Arc<AtomicUsize>,
    dropped: mpsc::Sender<()>,
}

impl Drop for DropChain {
    fn drop(&mut self) {
        assert!(
            !std::thread::panicking(),
            "each unwind is caught before another payload may be disposed"
        );
        if self.depth == 0 {
            self.result_drops.fetch_add(1, Ordering::SeqCst);
            self.dropped.send(()).unwrap();
        }
        if self.depth < 3 {
            panic_any(Self {
                depth: self.depth + 1,
                result_drops: Arc::clone(&self.result_drops),
                dropped: self.dropped.clone(),
            });
        }
        panic!("final opaque payload destructor");
    }
}

#[test]
fn remote_secondary_payload_drop_cannot_escape_result_disposal() {
    isolated("remote_secondary_payload_drop_cannot_escape_result_disposal", || {
        let runtime = runtime();
        let (release, gate) = events_once::Event::boxed();
        let (dropped, received) = mpsc::channel();
        let result_drops = Arc::new(AtomicUsize::new(0));
        let data = Unaware((gate, Arc::clone(&result_drops), dropped));
        let join = runtime
            .scheduler()
            .spawn_anywhere(data, |_, Unaware((gate, result_drops, dropped))| async move {
                gate.await.unwrap();
                Unaware(DropChain {
                    depth: 0,
                    result_drops,
                    dropped,
                })
            });
        drop(join);
        release.send(());
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).wait().unwrap(), 42);
        assert_eq!(result_drops.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}

#[test]
fn local_secondary_payload_drop_cannot_escape_result_disposal() {
    isolated("local_secondary_payload_drop_cannot_escape_result_disposal", || {
        let runtime = runtime();
        let (dropped, received) = mpsc::channel();
        let result_drops = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&result_drops);
        runtime
            .scheduler()
            .block_on(async move |cx| {
                let (release, gate) = events_once::Event::boxed();
                let (finished, completed) = events_once::Event::boxed();
                let join = cx.local_scheduler().unwrap().spawn(async move || {
                    gate.await.unwrap();
                    finished.send(());
                    DropChain {
                        depth: 0,
                        result_drops: captured,
                        dropped,
                    }
                });
                drop(join);
                release.send(());
                completed.await.unwrap();
                assert_eq!(
                    *cx.local_scheduler().unwrap().spawn(async || std::rc::Rc::new(42)).await.unwrap(),
                    42
                );
            })
            .unwrap();
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(result_drops.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}
