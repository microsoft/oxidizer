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

use arty_executor::{
    ExecutorBuilder, PanicPolicy, ShutdownCause, TaskBuilder, TaskCompletion, TaskIdentity, TaskInstrumentation, TaskSet,
    discard_task_panic_payload,
};
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
fn identities_preserve_locations_and_allocate_distinct_task_ids() {
    let location = std::panic::Location::caller();
    let first = TaskIdentity::from_location(location);
    let second = TaskIdentity::from_location(location);
    let other = TaskIdentity::from_location(std::panic::Location::caller());

    assert_ne!(first.id(), second.id());
    assert_eq!(first.descriptor(), second.descriptor());
    assert_eq!(first.descriptor().get(), std::ptr::from_ref(location).addr() as u64);
    assert!(std::ptr::eq(first.location(), location));
    assert_ne!(first.descriptor(), other.descriptor());
}

#[test]
fn current_identity_follows_each_poll_and_child_submission_without_monitoring() {
    let mut executor = ExecutorBuilder::new().build();
    let tasks = executor.tasks();
    let current = tasks.clone();
    let sibling_tasks = tasks.clone();
    let parent = TaskIdentity::from_location(std::panic::Location::caller());
    let child = TaskIdentity::from_location(std::panic::Location::caller());
    let sibling = TaskIdentity::from_location(std::panic::Location::caller());
    let polls = Rc::new(Cell::new(0));
    let polled = Rc::clone(&polls);
    let child_polled = Rc::new(Cell::new(false));
    let completed = Rc::clone(&child_polled);
    let sibling_polled = Rc::new(Cell::new(false));
    let sibling_completed = Rc::clone(&sibling_polled);
    let mut first = true;

    assert_eq!(tasks.current(), None);
    executor
        .tasks()
        .task(
            parent,
            poll_fn(move |cx| {
                assert_eq!(current.current(), Some(parent));
                polled.set(polled.get() + 1);
                if first {
                    first = false;
                    let completed = Rc::clone(&completed);
                    let child_tasks = tasks.clone();
                    tasks
                        .task(child, async move {
                            assert_eq!(child_tasks.current(), Some(child));
                            completed.set(true);
                        })
                        .register(DiscardCompletion);
                    assert_eq!(current.current(), Some(parent));
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }),
        )
        .instrument(())
        .inspect(Some)
        .register(DiscardCompletion);
    executor
        .tasks()
        .task(sibling, async move {
            assert_eq!(sibling_tasks.current(), Some(sibling));
            sibling_completed.set(true);
        })
        .register(DiscardCompletion);
    assert_eq!(executor.tasks().current(), None);

    let _ = executor.execute_cycle();
    assert_eq!(executor.tasks().current(), None);
    let _ = executor.execute_cycle();

    assert_eq!(executor.tasks().current(), None);
    assert_eq!(polls.get(), 2);
    assert!(child_polled.get());
    assert!(sibling_polled.get());
    assert!(!executor.has_tasks());
}

#[test]
fn current_identity_clears_after_task_panic_and_cancellation() {
    let mut executor = ExecutorBuilder::new().panic_policy(PanicPolicy::IGNORE).build();
    let identity = TaskIdentity::from_location(std::panic::Location::caller());
    let failed = Rc::new(Cell::new(false));
    let panicked = Rc::clone(&failed);
    let tasks = executor.tasks();
    executor
        .tasks()
        .task(
            identity,
            poll_fn(move |_| -> Poll<()> {
                assert_eq!(tasks.current(), Some(identity));
                panicked.set(true);
                panic!("task identity is cleared after a panic");
            }),
        )
        .register(DiscardCompletion);

    let _ = executor.execute_cycle();

    assert!(failed.get());
    assert_eq!(executor.tasks().current(), None);
    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .register(DiscardCompletion);
    let _ = executor.execute_cycle();
    executor.shutdown().trigger(ShutdownCause::default());
    let _ = executor.execute_cycle();
    assert_eq!(executor.tasks().current(), None);
    assert!(!executor.has_tasks());
}

#[test]
fn current_identity_clears_when_poll_instrumentation_unwinds() {
    struct PanickingInstrumentation(TaskIdentity, TaskSet);

    impl TaskInstrumentation for PanickingInstrumentation {
        fn poll_start(&mut self) {
            assert_eq!(self.1.current(), Some(self.0));
            panic!("poll instrumentation unwinds");
        }
    }

    let mut executor = ExecutorBuilder::new().build();
    let identity = TaskIdentity::from_location(std::panic::Location::caller());
    executor
        .tasks()
        .task(identity, std::future::pending::<()>())
        .instrument(PanickingInstrumentation(identity, executor.tasks()))
        .register(DiscardCompletion);

    let result = catch_unwind(std::panic::AssertUnwindSafe(|| executor.execute_cycle()));

    assert_eq!(*result.unwrap_err().downcast::<&str>().unwrap(), "poll instrumentation unwinds");
    assert_eq!(executor.tasks().current(), None);
}

#[test]
fn instrumentation_receives_native_identity_for_inspection_and_all_outcomes() {
    struct IdentityCallbacks(Rc<RefCell<Vec<(&'static str, TaskIdentity)>>>);

    impl TaskInstrumentation for IdentityCallbacks {
        fn inspect(&self, identity: &TaskIdentity, _state: arty_executor::TaskState, visitor: &mut dyn FnMut(&dyn std::any::Any)) {
            visitor(identity);
        }

        fn completed(&mut self, identity: &TaskIdentity) {
            self.0.borrow_mut().push(("completed", *identity));
        }

        fn panicked(&mut self, identity: &TaskIdentity) {
            self.0.borrow_mut().push(("panicked", *identity));
        }

        fn cancelled(&mut self, identity: &TaskIdentity) {
            self.0.borrow_mut().push(("cancelled", *identity));
        }
    }

    for (outcome, expected) in [(0, "completed"), (1, "panicked"), (2, "cancelled"), (3, "cancelled")] {
        let mut executor = ExecutorBuilder::new().panic_policy(PanicPolicy::IGNORE).build();
        let identity = TaskIdentity::from_location(std::panic::Location::caller());
        let callbacks = Rc::new(RefCell::new(Vec::new()));
        if outcome == 3 {
            executor.shutdown().trigger(ShutdownCause::default());
        }
        executor
            .tasks()
            .task(
                identity,
                poll_fn(move |_| match outcome {
                    0 => Poll::Ready(()),
                    1 => panic!("task panic retains its identity"),
                    _ => Poll::Pending,
                }),
            )
            .instrument(IdentityCallbacks(Rc::clone(&callbacks)))
            .inspect(Some)
            .register(DiscardCompletion);
        let mut inspected = Vec::new();
        executor.inspect(1, |identity: &TaskIdentity| inspected.push(*identity)).unwrap();
        assert_eq!(inspected, if outcome == 3 { vec![] } else { vec![identity] });
        assert_eq!(executor.tasks().current(), None);

        let _ = executor.execute_cycle();
        if outcome == 2 {
            executor.shutdown().trigger(ShutdownCause::default());
            let _ = executor.execute_cycle();
        }

        assert_eq!(*callbacks.borrow(), [(expected, identity)]);
        assert_eq!(executor.tasks().current(), None);
    }
}

#[test]
fn current_identity_belongs_to_the_task_set_instance() {
    let executor = ExecutorBuilder::new().build();
    let retired_tasks = executor.tasks();
    drop(executor);
    let mut executor = ExecutorBuilder::new().build();
    let tasks = executor.tasks();
    let identity = TaskIdentity::from_location(std::panic::Location::caller());
    let observed = Rc::new(Cell::new(false));
    let polled = Rc::clone(&observed);
    executor
        .tasks()
        .task(identity, async move {
            assert_eq!(retired_tasks.current(), None);
            assert_eq!(tasks.current(), Some(identity));
            polled.set(true);
        })
        .register(DiscardCompletion);

    let _ = executor.execute_cycle();

    assert!(observed.get());
    assert_eq!(executor.tasks().current(), None);
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
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                poll_fn(|_| -> Poll<()> { panic!("configured panic policy") }),
            )
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
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            poll_fn(move |cx| {
                if first {
                    first = false;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }),
        )
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
    fn cancelled(&mut self, _identity: &TaskIdentity) {
        panic!("instrumentation cancellation");
    }
}

enum TerminalPanic {
    Completed,
    Panicked,
}

impl TaskInstrumentation for TerminalPanic {
    fn completed(&mut self, _identity: &TaskIdentity) {
        if matches!(self, Self::Completed) {
            panic!("instrumentation completion");
        }
    }

    fn panicked(&mut self, _identity: &TaskIdentity) {
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
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                poll_fn(move |_| {
                    assert!(!future_panics, "task panic");
                    Poll::Ready(())
                }),
            )
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
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            CaptureWaker {
                waker: waker_tx,
                dropped: dropped_tx,
            },
        )
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
