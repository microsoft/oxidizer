// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::panic, clippy::unwrap_used, reason = "test code")]

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic::{RefUnwindSafe, UnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

use arty_executor::{ExecutorBuilder, PanicPolicy, TaskBuilder, TaskObserver, TaskSet, discard_task_panic_payload};
use static_assertions::assert_not_impl_any;

use super::support::{DiscardCompletion, TaskBuilderExt as _};

assert_not_impl_any!(TaskSet: Send, Sync);
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
        (ExecutorBuilder::new().panic_policy(PanicPolicy::IGNORE), false),
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

struct PollOutcomes {
    starts: Rc<Cell<usize>>,
    outcomes: Rc<RefCell<Vec<Poll<()>>>>,
}

impl TaskObserver for PollOutcomes {
    fn poll_start(&mut self) {
        self.starts.set(self.starts.get() + 1);
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.outcomes.borrow_mut().push(outcome);
    }
}

#[test]
fn task_observer_receives_each_poll_outcome() {
    let mut executor = ExecutorBuilder::new().build();
    let starts = Rc::new(Cell::new(0));
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let mut first = true;
    executor
        .tasks()
        .task(poll_fn(move |cx| {
            if first {
                first = false;
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        }))
        .observe(PollOutcomes {
            starts: Rc::clone(&starts),
            outcomes: Rc::clone(&outcomes),
        })
        .register(DiscardCompletion);

    _ = executor.execute_cycle();
    _ = executor.execute_cycle();

    assert_eq!(starts.get(), 2);
    assert_eq!(*outcomes.borrow(), [Poll::Pending, Poll::Ready(())]);
}

#[test]
fn configured_thread_must_identify_the_current_thread() {
    let foreign_id = std::thread::spawn(|| std::thread::current().id()).join().unwrap();
    let thread = thread_aware_core::__private::v1::new_thread(
        thread_aware_core::__private::v1::new_owner(),
        foreign_id,
        thread_aware_core::__private::v1::new_numa_node(0),
    );

    catch_unwind(|| ExecutorBuilder::new().thread(thread).build()).unwrap_err();
}

#[test]
fn configured_current_thread_builds_the_executor() {
    let thread = thread_aware_core::__private::v1::new_thread(
        thread_aware_core::__private::v1::new_owner(),
        std::thread::current().id(),
        thread_aware_core::__private::v1::new_numa_node(0),
    );

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
