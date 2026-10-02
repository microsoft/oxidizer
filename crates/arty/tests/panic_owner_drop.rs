// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime owner destruction during a task unwind must not join or panic on that task's thread.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::cell::RefCell;
use std::sync::mpsc;

use panic_support::{isolated, runtime};
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct Exited(mpsc::Sender<()>);

impl Drop for Exited {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

thread_local! {
    static EXIT: RefCell<Option<Exited>> = const { RefCell::new(None) };
}

struct Canary(mpsc::Sender<()>);

impl Drop for Canary {
    fn drop(&mut self) {
        self.0.send(()).unwrap();
    }
}

fn owner_unwind(blocking: bool) {
    let runtime = runtime();
    let (exited, retired) = mpsc::channel();
    let scheduler = runtime
        .scheduler()
        .block_on(async move |cx| {
            EXIT.with_borrow_mut(|exit| *exit = Some(Exited(exited)));
            cx.scheduler().clone()
        })
        .unwrap();
    let (started, ready) = mpsc::channel();
    let (dropped, cancelled) = mpsc::channel();
    let canary = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, dropped)), |_, Unaware((started, dropped))| async move {
            let guard = Canary(dropped);
            started.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(guard);
        });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let failed: arty::task::JoinHandle<()> = if blocking {
        scheduler.spawn_blocking(move || {
            let _owner = runtime;
            panic!("blocking callback owning runtime");
        })
    } else {
        scheduler.spawn(async move |_| {
            let _owner = runtime;
            panic!("async task owning runtime");
        })
    };
    assert!(failed.wait().unwrap_err().is_panic());
    cancelled.recv_timeout(TEST_TIMEOUT).unwrap();
    assert!(canary.wait().unwrap_err().is_shutdown());
    assert!(scheduler.spawn(async |_| 42).wait().unwrap_err().is_shutdown());
    retired.recv_timeout(TEST_TIMEOUT).unwrap();
}

#[test]
fn async_owner_drop_during_user_panic_requests_independent_shutdown() {
    isolated("async_owner_drop_during_user_panic_requests_independent_shutdown", || {
        owner_unwind(false);
    });
}

#[test]
fn blocking_owner_drop_during_user_panic_requests_independent_shutdown() {
    isolated("blocking_owner_drop_during_user_panic_requests_independent_shutdown", || {
        owner_unwind(true);
    });
}
