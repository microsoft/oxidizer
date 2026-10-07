// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking shutdown must join running work and discard queued captures safely.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use arty::runtime::RuntimeOperations;
use panic_support::runtime;
use testing_aids::TEST_TIMEOUT;

testing_aids::init_tracing!();

struct CaptureDropPanic {
    drops: Arc<AtomicUsize>,
    dropped: mpsc::Sender<()>,
}

impl Drop for CaptureDropPanic {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        self.dropped.send(()).unwrap();
        assert!(!std::thread::panicking());
        panic!("discarded blocking capture destructor");
    }
}

#[test]
fn queued_blocking_capture_drop_panic_still_completes_its_join() {
    let runtime = runtime();
    let operations = RuntimeOperations::from(&runtime);
    let (started, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let running = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        gate.recv_timeout(TEST_TIMEOUT).unwrap();
        42
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let invoked = Arc::new(AtomicUsize::new(0));
    let capture = CaptureDropPanic {
        drops: Arc::clone(&drops),
        dropped,
    };
    let captured_invoked = Arc::clone(&invoked);
    let queued = runtime.scheduler().spawn_blocking(move || {
        captured_invoked.fetch_add(1, Ordering::SeqCst);
        drop(capture);
    });
    operations.request_stop();
    release.send(()).unwrap();
    assert_eq!(running.wait().unwrap(), 42);
    runtime.stop().unwrap();
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
    assert!(queued.wait().unwrap_err().is_shutdown());
}

#[test]
fn running_blocking_capture_drop_panic_is_a_join_error() {
    let runtime = runtime();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let capture = CaptureDropPanic {
        drops: Arc::clone(&drops),
        dropped,
    };
    let task = runtime.scheduler().spawn_blocking(move || drop(capture));
    assert!(task.wait().unwrap_err().is_panic());
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scheduler().spawn_blocking(|| 7).wait().unwrap(), 7);
    runtime.stop().unwrap();
}

#[test]
fn stop_waits_for_running_blocking_panic_and_retains_its_error() {
    let runtime = runtime();
    let operations = RuntimeOperations::from(&runtime);
    let (started, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (finished, observed) = mpsc::channel();
    let task: arty::task::JoinHandle<()> = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        gate.recv_timeout(TEST_TIMEOUT).unwrap();
        finished.send(()).unwrap();
        panic!("running blocking callback at shutdown");
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    operations.request_stop();
    release.send(()).unwrap();
    runtime.stop().unwrap();
    observed.recv_timeout(TEST_TIMEOUT).unwrap();
    assert!(task.wait().unwrap_err().is_panic());
}
