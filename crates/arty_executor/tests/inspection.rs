// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit native inspection must not add callbacks to ordinary execution.

#![cfg(feature = "inspection")]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use arty_executor::{
    Executor, InspectionError, InspectionStatus, PollObserver, ShutdownCause, TaskCompletion, TaskOptions, TaskPanicPayload, TaskState,
};

#[derive(Debug)]
struct Completion;

impl<R: 'static> TaskCompletion<R> for Completion {
    fn completed(self, _result: R) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[test]
fn ordinary_cycles_never_construct_diagnostic_descriptions() {
    let mut executor = Executor::builder().build();
    let calls = Rc::new(Cell::new(0));
    let inspected = Rc::clone(&calls);
    executor
        .tasks()
        .add_with_inspection(std::future::pending::<()>(), TaskOptions::default(), Completion, move |_, state| {
            inspected.set(inspected.get() + 1);
            Some(state)
        });

    let _ = executor.execute_cycle();

    assert_eq!(calls.get(), 0);
}

#[test]
fn inspection_reports_native_waiting_and_notified_state_without_consuming_wakes() {
    let mut executor = Executor::builder().build();
    let waker = Rc::new(RefCell::new(None::<Waker>));
    let captured = Rc::clone(&waker);
    let future = std::future::poll_fn(move |cx: &mut Context<'_>| {
        *captured.borrow_mut() = Some(cx.waker().clone());
        Poll::<()>::Pending
    });
    executor
        .tasks()
        .add_with_inspection(future, TaskOptions::default(), Completion, |_, state| Some(state));
    let _ = executor.execute_cycle();
    let mut before = Vec::new();
    executor.inspect(10, |state: &TaskState| before.push(*state)).unwrap();
    waker.borrow().as_ref().unwrap().wake_by_ref();

    let mut notified = Vec::new();
    executor.inspect(10, |state: &TaskState| notified.push(*state)).unwrap();
    let _ = executor.execute_cycle();
    let mut after = Vec::new();
    executor.inspect(10, |state: &TaskState| after.push(*state)).unwrap();

    assert_eq!(
        (before, notified, after),
        (vec![TaskState::Waiting], vec![TaskState::Ready], vec![TaskState::Waiting])
    );
}

#[test]
fn inspection_counts_undescribed_tasks_toward_the_work_bound() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    tasks.add(std::future::pending::<()>(), TaskOptions::default(), Completion);
    tasks.add_with_inspection(std::future::pending::<()>(), TaskOptions::default(), Completion, |_, _| {
        Some(42_u32)
    });
    let mut described = Vec::new();

    let status = executor.inspect(1, |value: &u32| described.push(*value)).unwrap();

    assert_eq!((status, described), (InspectionStatus::Truncated, Vec::new()));
}

#[derive(Debug, Default)]
struct Observer {
    starts: Cell<usize>,
    finishes: Cell<usize>,
    errors: Cell<usize>,
}

impl PollObserver for Observer {
    fn poll_started(&self, description: &dyn Any) {
        assert_eq!(description.downcast_ref::<u32>(), Some(&42));
        self.starts.set(self.starts.get() + 1);
    }

    fn poll_finished(&self, completed: bool) {
        assert!(completed);
        self.finishes.set(self.finishes.get() + 1);
    }

    fn inspection_failed(&self, error: InspectionError) {
        assert_eq!(error, InspectionError::Panicked);
        self.errors.set(self.errors.get() + 1);
    }
}

#[test]
fn observed_polls_pair_callbacks_only_for_described_tasks() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    tasks.add(async {}, TaskOptions::default(), Completion);
    tasks.add_with_inspection(async {}, TaskOptions::default(), Completion, |_, _| Some(42_u32));
    let observer = Observer::default();

    let _ = executor.execute_cycle_observed(&observer);

    assert_eq!((observer.starts.get(), observer.finishes.get(), observer.errors.get()), (1, 1, 0));
}

#[test]
fn a_panicking_description_does_not_prevent_its_task_from_running() {
    let mut executor = Executor::builder().build();
    let ran = Rc::new(Cell::new(false));
    let signal = Rc::clone(&ran);
    executor.tasks().add_with_inspection(
        async move {
            signal.set(true);
        },
        TaskOptions::default(),
        Completion,
        |_, _| -> Option<u32> {
            panic!("broken inspector");
        },
    );
    let observer = Observer::default();

    let _ = executor.execute_cycle_observed(&observer);

    assert_eq!(
        (ran.get(), observer.starts.get(), observer.finishes.get(), observer.errors.get()),
        (true, 0, 0, 1)
    );
}

#[test]
fn a_panicking_description_drop_cancels_the_started_observation() {
    struct Description;
    impl Drop for Description {
        fn drop(&mut self) {
            panic!("description disposal failed");
        }
    }
    #[derive(Debug, Default)]
    struct Calls(RefCell<Vec<&'static str>>);
    impl PollObserver for Calls {
        fn poll_started(&self, _: &dyn Any) {
            self.0.borrow_mut().push("started");
        }
        fn poll_finished(&self, _: bool) {
            self.0.borrow_mut().push("finished");
        }
        fn inspection_failed(&self, _: InspectionError) {
            self.0.borrow_mut().push("failed");
        }
    }
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .add_with_inspection(async {}, TaskOptions::default(), Completion, |_, _| Some(Description));
    let observer = Calls::default();

    let _ = executor.execute_cycle_observed(&observer);

    assert_eq!(*observer.0.borrow(), ["started", "failed"]);
}
