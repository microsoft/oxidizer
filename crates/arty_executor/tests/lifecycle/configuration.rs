// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::panic, clippy::unwrap_used, reason = "test code")]

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic::{RefUnwindSafe, UnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::thread;

use arty_executor::{ExecutorBuilder, PanicPolicy, TaskBuilder, TaskCompletion, TaskInstrumentation, TaskSet, discard_task_panic_payload};
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

impl TaskInstrumentation for PollOutcomes {
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
        .instrument(PollOutcomes {
            starts: Rc::clone(&starts),
            outcomes: Rc::clone(&outcomes),
        })
        .register(DiscardCompletion);

    _ = executor.execute_cycle();
    _ = executor.execute_cycle();

    assert_eq!(starts.get(), 2);
    assert_eq!(*outcomes.borrow(), [Poll::Pending, Poll::Ready(())]);
}

struct PanickingCancellation;

impl TaskInstrumentation for PanickingCancellation {
    fn cancelled(&mut self) {
        panic!("instrumentation cancellation");
    }
}

enum TerminalPanic {
    Completed,
    Panicked,
}

impl TaskInstrumentation for TerminalPanic {
    fn completed(&mut self) {
        if matches!(self, Self::Completed) {
            panic!("instrumentation completion");
        }
    }

    fn panicked(&mut self) {
        if matches!(self, Self::Panicked) {
            panic!("instrumentation panic");
        }
    }
}

#[derive(Debug)]
struct RecordCompletion(Rc<Cell<Option<&'static str>>>);

impl TaskCompletion<()> for RecordCompletion {
    fn completed(self, (): ()) {
        self.0.set(Some("completed"));
    }

    fn panicked(self, panic: arty_executor::TaskPanicPayload, _cause: Option<arty_executor::ShutdownCause>) {
        discard_task_panic_payload(panic);
        self.0.set(Some("panicked"));
    }

    fn cancelled(self, _cause: arty_executor::ShutdownCause) {
        self.0.set(Some("cancelled"));
    }
}

#[test]
fn terminal_instrumentation_panics_after_completion_delivery() {
    for (future_panics, instrumentation, message, outcome) in [
        (false, TerminalPanic::Completed, "instrumentation completion", "completed"),
        (true, TerminalPanic::Panicked, "instrumentation panic", "panicked"),
    ] {
        let mut executor = ExecutorBuilder::new().panic_policy(PanicPolicy::IGNORE).build();
        let delivered = Rc::new(Cell::new(None));
        executor
            .tasks()
            .task(poll_fn(move |_| {
                assert!(!future_panics, "task panic");
                Poll::Ready(())
            }))
            .instrument(instrumentation)
            .register(RecordCompletion(Rc::clone(&delivered)));

        let panic = catch_unwind(std::panic::AssertUnwindSafe(|| executor.execute_cycle())).unwrap_err();

        assert_eq!(*panic.downcast::<&str>().unwrap(), message);
        assert_eq!(delivered.get(), Some(outcome));
    }
}

struct CaptureWaker {
    waker: std::sync::mpsc::Sender<Waker>,
    dropped: std::sync::mpsc::Sender<thread::ThreadId>,
}

impl Future for CaptureWaker {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.waker.send(cx.waker().clone()).unwrap();
        Poll::Pending
    }
}

impl Drop for CaptureWaker {
    fn drop(&mut self) {
        self.dropped.send(thread::current().id()).unwrap();
    }
}

#[test]
fn panicking_cancellation_retires_payload_before_remote_waker_release() {
    let owner = thread::current().id();
    let (waker_tx, waker_rx) = std::sync::mpsc::channel();
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    let mut executor = ExecutorBuilder::new().build();
    executor
        .tasks()
        .task(CaptureWaker {
            waker: waker_tx,
            dropped: dropped_tx,
        })
        .instrument(PanickingCancellation)
        .register(DiscardCompletion);
    _ = executor.execute_cycle();
    let waker = waker_rx.recv().unwrap();
    executor.shutdown().trigger(arty_executor::ShutdownCause::default());

    let panic = catch_unwind(std::panic::AssertUnwindSafe(|| executor.execute_cycle())).unwrap_err();
    assert_eq!(*panic.downcast::<&str>().unwrap(), "instrumentation cancellation");
    assert_eq!(dropped_rx.recv().unwrap(), owner);
    drop(executor);

    thread::spawn(move || drop(waker)).join().unwrap();
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
