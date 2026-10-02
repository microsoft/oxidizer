// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A dropped join neither cancels its task nor transfers task panics to the caller.

#![cfg(feature = "rt")]

mod panic_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use panic_support::{isolated, runtime};
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct ResultDropPanic {
    drops: Arc<AtomicUsize>,
    dropped: mpsc::Sender<()>,
}

impl Drop for ResultDropPanic {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        self.dropped.send(()).unwrap();
        panic!("unreceived result destructor");
    }
}

#[test]
fn abandoned_remote_result_drop_is_contained() {
    isolated("abandoned_remote_result_drop_is_contained", || {
        let runtime = runtime();
        let (release, gate) = events_once::Event::boxed();
        let (dropped, received) = mpsc::channel();
        let drops = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&drops);
        let task = runtime.scheduler().spawn_anywhere(
            Unaware((gate, captured, dropped)),
            |_, Unaware((gate, drops, dropped))| async move {
                gate.await.unwrap();
                Unaware(ResultDropPanic { drops, dropped })
            },
        );
        drop(task);
        release.send(());
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).wait().unwrap(), 7);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}

#[test]
fn abandoned_local_result_drop_is_contained() {
    isolated("abandoned_local_result_drop_is_contained", || {
        let runtime = runtime();
        let (dropped, received) = mpsc::channel();
        let drops = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&drops);
        runtime
            .scheduler()
            .block_on(async move |cx| {
                let (release, gate) = events_once::Event::boxed();
                let (done, completed) = events_once::Event::boxed();
                let task = cx.local_scheduler().unwrap().spawn(async move || {
                    gate.await.unwrap();
                    done.send(());
                    ResultDropPanic {
                        drops: captured,
                        dropped,
                    }
                });
                drop(task);
                release.send(());
                completed.await.unwrap();
                assert_eq!(*cx.local_scheduler().unwrap().spawn(async || std::rc::Rc::new(7)).await.unwrap(), 7);
            })
            .unwrap();
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}

#[test]
fn abandoned_blocking_result_drop_is_contained() {
    isolated("abandoned_blocking_result_drop_is_contained", || {
        let runtime = runtime();
        let (started, ready) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (dropped, received) = mpsc::channel();
        let drops = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&drops);
        let task = runtime.scheduler().spawn_blocking(move || {
            started.send(()).unwrap();
            gate.recv_timeout(TEST_TIMEOUT).unwrap();
            ResultDropPanic {
                drops: captured,
                dropped,
            }
        });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        drop(task);
        release.send(()).unwrap();
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(runtime.scheduler().spawn_blocking(|| 7).wait().unwrap(), 7);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}
