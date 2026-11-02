// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit native inspection must not add callbacks to ordinary execution.

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
    executor
        .tasks()
        .add_with_inspection(std::future::pending::<()>(), TaskOptions::default(), Completion, {
            let calls = Rc::clone(&calls);
            move |_, state| {
                calls.set(calls.get() + 1);
                Some(state)
            }
        });

    let _ = executor.execute_cycle();

    assert_eq!(calls.get(), 0);
}

#[test]
fn inspection_reports_native_waiting_and_notified_state_without_consuming_wakes() {
    let mut executor = Executor::builder().build();
    let waker = Rc::new(RefCell::new(None::<Waker>));
    let future = std::future::poll_fn({
        let waker = Rc::clone(&waker);
        move |cx: &mut Context<'_>| {
            *waker.borrow_mut() = Some(cx.waker().clone());
            Poll::<()>::Pending
        }
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
    tasks.add(std::future::pending::<()>(), TaskOptions::default(), Completion);
    let mut described = Vec::new();

    let status = executor.inspect(1, |value: &u32| described.push(*value)).unwrap();

    assert_eq!((status, described), (InspectionStatus::Truncated, Vec::new()));
}

#[test]
fn complete_inspection_visits_pending_tasks_and_enforces_its_limit() {
    let mut executor = Executor::builder().build();
    for id in 1..=3 {
        executor
            .tasks()
            .add_with_inspection(std::future::pending::<()>(), TaskOptions::default(), Completion, move |_, _| {
                Some(id)
            });
    }
    let _ = executor.execute_cycle();
    let mut visited = Vec::new();

    let complete = executor.inspect(3, |id: &i32| visited.push(*id)).unwrap();
    visited.sort_unstable();
    let mut limited = Vec::new();
    let truncated = executor.inspect(2, |id: &i32| limited.push(*id)).unwrap();

    assert_eq!(
        (complete, visited, truncated, limited.len()),
        (InspectionStatus::Complete, vec![1, 2, 3], InspectionStatus::Truncated, 2)
    );
}

#[test]
fn invalid_limits_and_panicking_visitors_report_errors() {
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .add_with_inspection(std::future::pending::<()>(), TaskOptions::default(), Completion, |_, _| {
            Some(42_u32)
        });

    let invalid = executor.inspect::<u32>(0, |_| {}).unwrap_err();
    let failed = executor.inspect::<u32>(1, |_| panic!("visitor failed")).unwrap_err();

    assert_eq!((invalid, failed), (InspectionError::InvalidLimit, InspectionError::Panicked));
    assert!(!invalid.to_string().is_empty());
    assert!(!failed.to_string().is_empty());
    assert!(!InspectionError::Busy.to_string().is_empty());
}

#[test]
fn inspector_panic_payload_disposal_is_contained_in_both_entry_points() {
    // Dispose an unexpectedly escaping payload before asserting, so its
    // destructor cannot panic again inside the test harness.
    fn contain<T>(action: impl FnOnce() -> T) -> Option<T> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
            Ok(result) => Some(result),
            Err(panic) => {
                arty_executor::discard_task_panic_payload(panic);
                None
            }
        }
    }
    #[derive(Debug)]
    struct PanicOnDrop;
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("panic payload disposal");
        }
    }
    let mut executor = Executor::builder().build();
    let ran = Rc::new(Cell::new(false));
    executor.tasks().add_with_inspection(
        {
            let ran = Rc::clone(&ran);
            async move {
                ran.set(true);
            }
        },
        TaskOptions::default(),
        Completion,
        |_, _| -> Option<u32> { std::panic::panic_any(PanicOnDrop) },
    );
    let observer = Observer::default();

    let inspected = contain(|| executor.inspect::<u32>(1, |_| {}));
    let polled = contain(|| executor.execute_cycle_observed(&observer));

    assert_eq!(
        (inspected, polled.is_some(), ran.get(), observer.errors.get()),
        (Some(Err(InspectionError::Panicked)), true, true, 1)
    );
}

#[test]
fn observer_panics_propagate_and_live_active_tasks_remain_inspectable() {
    #[derive(Debug)]
    struct Broken;
    impl PollObserver for Broken {
        fn poll_started(&self, _: &dyn Any) {
            std::panic::panic_any(71_u32);
        }
        fn poll_finished(&self, _: bool) {
            panic!("no poll started");
        }
        fn inspection_failed(&self, _: InspectionError) {
            panic!("observer failures must propagate");
        }
    }
    let mut executor = Executor::builder().build();
    for id in 1..=3 {
        executor
            .tasks()
            .add_with_inspection(std::future::pending::<()>(), TaskOptions::default(), Completion, move |_, _| {
                Some(id)
            });
    }

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| executor.execute_cycle_observed(&Broken))).unwrap_err();
    let mut visited = Vec::new();
    let status = executor.inspect(1, |id: &i32| visited.push(*id)).unwrap();

    assert_eq!(
        (panic.downcast_ref::<u32>(), status, visited.len()),
        (Some(&71), InspectionStatus::Truncated, 1)
    );
}

#[test]
fn a_woken_pending_task_is_inspected_once_after_an_observer_panics() {
    #[derive(Debug)]
    struct Broken;
    impl PollObserver for Broken {
        fn poll_started(&self, _: &dyn Any) {
            std::panic::panic_any(71_u32);
        }
        fn poll_finished(&self, _: bool) {
            panic!("no observed poll completed");
        }
        fn inspection_failed(&self, _: InspectionError) {
            panic!("observer failures must propagate");
        }
    }
    let mut executor = Executor::builder().build();
    let waker = Rc::new(RefCell::new(None::<Waker>));
    let future = std::future::poll_fn({
        let waker = Rc::clone(&waker);
        move |cx| {
            *waker.borrow_mut() = Some(cx.waker().clone());
            Poll::<()>::Pending
        }
    });
    executor
        .tasks()
        .add_with_inspection(future, TaskOptions::default(), Completion, |_, state| Some((7_u32, state)));
    let _ = executor.execute_cycle();
    waker.borrow().as_ref().unwrap().wake_by_ref();

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| executor.execute_cycle_observed(&Broken))).unwrap_err();
    let mut visited = Vec::new();
    let status = executor
        .inspect(1, |description: &(u32, TaskState)| visited.push(*description))
        .unwrap();

    assert_eq!(
        (panic.downcast_ref::<u32>(), status, visited),
        (Some(&71), InspectionStatus::Complete, vec![(7, TaskState::Ready)])
    );
}

#[test]
fn inspector_capture_drop_panics_preserve_each_terminal_notification() {
    #[derive(Clone, Copy, Debug)]
    enum Mode {
        Complete,
        Panicked,
        Cancel,
    }
    #[derive(Debug, Eq, PartialEq)]
    enum Terminal {
        Completed(u32),
        Panicked(Option<u32>),
        Cancelled,
    }
    #[derive(Debug)]
    struct Report(Rc<RefCell<Vec<Terminal>>>);
    impl TaskCompletion<u32> for Report {
        fn completed(self, result: u32) {
            self.0.borrow_mut().push(Terminal::Completed(result));
        }
        fn panicked(self, panic: TaskPanicPayload, _: Option<ShutdownCause>) {
            self.0.borrow_mut().push(Terminal::Panicked(panic.downcast_ref::<u32>().copied()));
            arty_executor::discard_task_panic_payload(panic);
        }
        fn cancelled(self, _: ShutdownCause) {
            self.0.borrow_mut().push(Terminal::Cancelled);
        }
    }
    struct Capture(Rc<Cell<usize>>);
    impl Drop for Capture {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
            panic!("inspector capture destructor");
        }
    }
    for (mode, expected) in [
        (Mode::Complete, Terminal::Completed(42)),
        (Mode::Panicked, Terminal::Panicked(Some(51))),
        (Mode::Cancel, Terminal::Cancelled),
    ] {
        let mut executor = Executor::builder().build();
        let terminal = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let future = std::future::poll_fn(move |_| match mode {
            Mode::Complete => Poll::Ready(42),
            Mode::Panicked => std::panic::panic_any(51_u32),
            Mode::Cancel => Poll::Pending,
        });
        executor
            .tasks()
            .add_with_inspection(future, TaskOptions::default(), Report(Rc::clone(&terminal)), {
                let capture = Capture(Rc::clone(&drops));
                move |_, _| {
                    let _keep = &capture;
                    Some(())
                }
            });

        let retired = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !matches!(mode, Mode::Cancel) {
                let _ = executor.execute_cycle();
            }
            drop(executor);
        }));

        assert_eq!(
            (retired.is_ok(), drops.get(), std::mem::take(&mut *terminal.borrow_mut())),
            (true, 1, vec![expected]),
            "{mode:?}"
        );
    }
}

#[test]
fn submissions_with_inspection_after_shutdown_report_cancellation_without_polling() {
    #[derive(Debug)]
    struct Canceled(Rc<Cell<bool>>);
    impl TaskCompletion<()> for Canceled {
        fn completed(self, (): ()) {
            panic!("a stopped executor must not run this task");
        }
        fn panicked(self, panic: TaskPanicPayload, _: Option<ShutdownCause>) {
            arty_executor::discard_task_panic_payload(panic);
        }
        fn cancelled(self, _: ShutdownCause) {
            self.0.set(true);
        }
    }
    let executor = Executor::builder().build();
    let canceled = Rc::new(Cell::new(false));
    executor.shutdown().trigger(ShutdownCause::default());

    executor.tasks().add_with_inspection(
        std::future::pending::<()>(),
        TaskOptions::default(),
        Canceled(Rc::clone(&canceled)),
        |_, _| Some(42_u32),
    );

    assert!(canceled.get());
}

#[test]
fn inspection_value_types_preserve_thread_and_unwind_contracts() {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;
    assert_impl_all!(TaskState: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InspectionStatus: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InspectionError: Send, Sync, UnwindSafe, RefUnwindSafe);
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
    executor.tasks().add_with_inspection(
        {
            let ran = Rc::clone(&ran);
            async move {
                ran.set(true);
            }
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
