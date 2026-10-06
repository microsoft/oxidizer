// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Escaped timer callbacks cannot release storage referenced by live task wakers.

#![cfg(feature = "rt")]
#![cfg(test)]
#![cfg(not(miri))] // The regression isolates intentional process termination in a native child.

testing_aids::init_tracing!();

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use arty::runtime::{CpuPolicy, Runtime};
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

const CHILD: &str = "ARTY_WORKER_PANIC_BOUNDARY_CHILD";
const MESSAGE: &str = "timer callback crossed the task boundary";

struct PanicWake;

impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        panic!("{MESSAGE}");
    }
}

struct WorkerExited(mpsc::Sender<()>);

impl Drop for WorkerExited {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

thread_local! {
    static EXIT: RefCell<Option<WorkerExited>> = const { RefCell::new(None) };
}

#[test]
fn timer_panic_cannot_unwind_past_live_executor_storage() {
    if std::env::var_os(CHILD).is_some() {
        let runtime = Runtime::builder().cpu_policy(CpuPolicy::exactly(1)).build().unwrap();
        let (sent, received) = mpsc::channel();
        let (exited, worker_exited) = mpsc::channel();
        let _task = runtime
            .scheduler()
            .spawn_anywhere(Unaware((sent, exited)), |cx, Unaware((sent, exited))| async move {
                EXIT.with_borrow_mut(|exit| *exit = Some(WorkerExited(exited)));
                let mut sent = Some(sent);
                poll_fn(move |context| {
                    if let Some(sent) = sent.take() {
                        sent.send(context.waker().clone()).unwrap();
                        let mut timer = cx.clock().delay(Duration::from_millis(1));
                        let waker = Waker::from(Arc::new(PanicWake));
                        assert!(Pin::new(&mut timer).poll(&mut Context::from_waker(&waker)).is_pending());
                        // A poisoned timer lock would make its destructor mask the lifecycle failure.
                        std::mem::forget(timer);
                    }
                    Poll::<()>::Pending
                })
                .await;
            });
        let retained = received.recv_timeout(TEST_TIMEOUT).unwrap();
        if worker_exited.recv_timeout(TEST_TIMEOUT).is_err() {
            // A hang is not the required fail-closed termination. Leak objects that
            // may still reference the worker and let the child exit successfully so
            // the parent rejects this outcome.
            std::mem::forget(retained);
            std::mem::forget(runtime);
            return;
        }
        // The boundary must terminate before TLS teardown. Do not dereference a stale waker.
        std::mem::forget(retained);
        std::mem::forget(runtime);
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("timer_panic_cannot_unwind_past_live_executor_storage")
        .arg("--nocapture")
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(MESSAGE));
}
