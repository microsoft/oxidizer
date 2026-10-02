// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Accepted factories remain task-owned until invocation or shutdown disposal.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arty::runtime::RuntimeOperations;
use panic_support::{isolated, runtime};

testing_aids::init_tracing!();

struct CaptureDropPanic(Arc<AtomicUsize>);

impl Drop for CaptureDropPanic {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert!(!std::thread::panicking());
        panic!("factory capture destructor");
    }
}

#[test]
fn queued_remote_factory_drop_panic_does_not_interrupt_shutdown() {
    isolated("queued_remote_factory_drop_panic_does_not_interrupt_shutdown", || {
        let runtime = runtime();
        let drops = Arc::new(AtomicUsize::new(0));
        let invoked = Arc::new(AtomicUsize::new(0));
        let capture = CaptureDropPanic(Arc::clone(&drops));
        let captured_invoked = Arc::clone(&invoked);
        #[expect(
            clippy::async_yields_async,
            reason = "the join must be observed after root destruction and shutdown, not awaited inside the root"
        )]
        let queued = runtime
            .scheduler()
            .block_on(async move |cx| {
                let queued = cx.scheduler().spawn(move |_| {
                    captured_invoked.fetch_add(1, Ordering::SeqCst);
                    drop(capture);
                    std::future::ready(())
                });
                RuntimeOperations::from(&cx).request_stop();
                queued
            })
            .unwrap();
        runtime.stop().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(invoked.load(Ordering::SeqCst), 0);
        assert!(queued.wait().unwrap_err().is_shutdown());
    });
}

#[test]
fn remote_and_local_factory_capture_drop_panics_are_join_errors() {
    isolated("remote_and_local_factory_capture_drop_panics_are_join_errors", || {
        let runtime = runtime();
        let drops = Arc::new(AtomicUsize::new(0));
        let remote_capture = CaptureDropPanic(Arc::clone(&drops));
        let local_capture = CaptureDropPanic(Arc::clone(&drops));
        runtime
            .scheduler()
            .block_on(async move |cx| {
                let remote = cx.scheduler().spawn(move |_| {
                    drop(remote_capture);
                    std::future::ready(())
                });
                assert!(remote.await.unwrap_err().is_panic());
                let value = Rc::new(42);
                let local = cx.local_scheduler().unwrap().spawn(move || {
                    assert_eq!(*value, 42);
                    drop(local_capture);
                    std::future::ready(())
                });
                assert!(local.await.unwrap_err().is_panic());
                assert_eq!(cx.scheduler().spawn(async |_| 7).await.unwrap(), 7);
            })
            .unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        runtime.stop().unwrap();
    });
}
