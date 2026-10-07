// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macro roots resume their original panic only after owned task teardown.

#![cfg(feature = "macros")]
#![cfg(test)]

mod panic_support;

use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::atomic::{AtomicUsize, Ordering};

testing_aids::init_tracing!();

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct RootPayload(u32);

struct Child;

impl Drop for Child {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

#[arty::main(workers = 1)]
async fn root(cx: arty::task::Builtins) {
    let (started, ready) = events_once::Event::boxed();
    let task = cx.local_scheduler().unwrap().spawn(async move || {
        let guard = Child;
        started.send(());
        std::future::pending::<()>().await;
        drop(guard);
    });
    drop(task);
    ready.await.unwrap();
    panic_any(RootPayload(42));
}

#[test]
fn macro_original_payload_is_resumed_after_local_child_teardown() {
    let panic = catch_unwind(AssertUnwindSafe(root)).unwrap_err();
    assert_eq!(panic.downcast_ref::<RootPayload>().unwrap().0, 42);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    panic_support::runtime().stop().unwrap();
}
