// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::panic::{RefUnwindSafe, UnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

use arty_executor::{ExecutorBuilder, PanicPolicy, TaskBuilder, TaskPlacement, TaskSet, discard_task_panic_payload};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadBuilder;

use crate::support::TaskBuilderExt as _;

assert_not_impl_any!(TaskSet: Send, Sync);
assert_impl_all!(TaskPlacement: UnwindSafe, RefUnwindSafe);
// Captured enrichment can contain user-defined interior state.
assert_not_impl_any!(TaskBuilder<'static, std::future::Ready<()>>: UnwindSafe, RefUnwindSafe);

#[test]
fn default_builder() {
    _ = ExecutorBuilder::default();
}

#[test]
fn panic_policy_is_configurable() {
    for (builder, shuts_down) in [
        (ExecutorBuilder::new(), true),
        (ExecutorBuilder::new().panic_policy(PanicPolicy::Ignore), false),
    ] {
        let mut executor = builder.build();
        let _completion = executor
            .tasks()
            .task(poll_fn(|_| -> Poll<()> { panic!("configured panic policy") }))
            .register_test();

        _ = executor.execute_cycle();

        assert_eq!(executor.shutdown().is_triggered(), shuts_down);
    }
}

#[test]
fn configured_thread_must_identify_the_current_thread() {
    let foreign_id = std::thread::spawn(|| std::thread::current().id()).join().unwrap();
    let thread = ThreadBuilder::default().build(foreign_id);

    catch_unwind(|| ExecutorBuilder::new().thread(thread).build()).unwrap_err();
}

#[test]
fn configured_current_thread_builds_the_executor() {
    let thread = ThreadBuilder::default().build(std::thread::current().id());

    drop(ExecutorBuilder::new().thread(thread).build());
}

struct PanicOnce(Arc<AtomicUsize>);

#[cfg(test)]
impl Drop for PanicOnce {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
        panic!("panic payload destructor");
    }
}

#[test]
fn captured_panic_with_panicking_destructor_is_discarded() {
    let drops = Arc::new(AtomicUsize::new(0));
    let result = catch_unwind(std::panic::AssertUnwindSafe({
        let drops = Arc::clone(&drops);
        move || discard_task_panic_payload(Box::new(PanicOnce(drops)))
    }));

    result.unwrap();
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}
