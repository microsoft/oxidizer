// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::panic, clippy::unwrap_used, reason = "test code")]

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic::{RefUnwindSafe, UnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use arty_executor::{ExecutorBuilder, PanicPolicy, ShutdownCause, TaskBuilder, TaskObserver, TaskSet, discard_task_panic_payload};
use seismograph_runtime::TaskIdentity;
use static_assertions::assert_not_impl_any;

use super::support::{DiscardCompletion, TaskBuilderExt as _};

assert_not_impl_any!(TaskSet: Send, Sync);
// Captured enrichment can contain user-defined interior state.
assert_not_impl_any!(TaskBuilder<'static, std::future::Ready<()>>: UnwindSafe, RefUnwindSafe);

static RECORDING: Mutex<()> = Mutex::new(());

#[test]
fn default_builder() {
    _ = ExecutorBuilder::default();
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
        .observe(())
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
fn task_runs_without_false_cancellation_when_executor_monitoring_is_absent() {
    let _recording = RECORDING.lock().unwrap();
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let runtime = seismograph_runtime::Runtime::builder("executor-without-monitoring").build();
    let identity = TaskIdentity::from_location(std::panic::Location::caller());
    let queued = runtime.async_task(identity, 0).unwrap().enqueued(0);
    let completed = Rc::new(Cell::new(false));
    let observed = Rc::clone(&completed);
    let mut executor = ExecutorBuilder::new().build();
    executor
        .tasks()
        .task(identity, async move {
            observed.set(true);
        })
        .monitor(queued)
        .register(DiscardCompletion);

    let _ = executor.execute_cycle();

    let events = seismograph::recorder::current_thread_events().unwrap();
    seismograph::recorder(seismograph::recorder::Configuration::default());
    assert!(completed.get());
    assert!(!events.iter().any(|event| {
        event.kind == seismograph::recorder::event::EventKind::TaskCanceled
            && event.runtime().is_some_and(|payload| payload.subject_id == identity.id().get())
    }));
}

#[test]
fn task_created_without_monitoring_emits_poll_events_after_recording_starts() {
    let _recording = RECORDING.lock().unwrap();
    seismograph::recorder(seismograph::recorder::Configuration::default());
    let runtime = seismograph_runtime::Runtime::builder("late-recording").build();
    let worker = runtime.async_worker(seismograph_runtime::WorkerMetadata::new(seismograph_runtime::WorkerRole::Core));
    let identity = TaskIdentity::from_location(std::panic::Location::caller());
    let mut executor = ExecutorBuilder::new().monitoring(worker).build();
    executor.tasks().task(identity, std::future::ready(())).register(DiscardCompletion);
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });

    let _ = executor.execute_cycle();

    let kinds = seismograph::recorder::current_thread_events()
        .unwrap()
        .into_iter()
        .filter(|event| event.runtime().is_some_and(|payload| payload.subject_id == identity.id().get()))
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    seismograph::recorder(seismograph::recorder::Configuration::default());
    assert_eq!(
        kinds,
        [
            seismograph::recorder::event::EventKind::TaskPollStarted,
            seismograph::recorder::event::EventKind::TaskPollFinished,
        ]
    );
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

    impl TaskObserver for PanickingInstrumentation {
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
        .observe(PanickingInstrumentation(identity, executor.tasks()))
        .register(DiscardCompletion);

    let result = catch_unwind(std::panic::AssertUnwindSafe(|| executor.execute_cycle()));

    assert_eq!(*result.unwrap_err().downcast::<&str>().unwrap(), "poll instrumentation unwinds");
    assert_eq!(executor.tasks().current(), None);
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
