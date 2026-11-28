// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit native inspection must not add callbacks to ordinary execution.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use arty_executor::{
    Executor, InspectionError, InspectionStatus, PollObserver, Shutdown, ShutdownCause, TaskCompletion, TaskPanicPayload, TaskState,
};
use seismograph_runtime::TaskIdentity;

#[derive(Debug)]
struct Completion;

impl<R: 'static> TaskCompletion<R> for Completion {
    fn completed(self, _result: R) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[derive(Debug)]
struct InspectedCompletion {
    inspections: Rc<Cell<u32>>,
    outcome: Rc<Cell<Option<(&'static str, u32)>>>,
}

impl TaskCompletion<()> for InspectedCompletion {
    fn completed(self, (): ()) {
        self.outcome.set(Some(("completed", self.inspections.get())));
    }

    fn panicked(self, panic: TaskPanicPayload, _: Option<ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
        self.outcome.set(Some(("panicked", self.inspections.get())));
    }

    fn cancelled(self, _: ShutdownCause) {
        self.outcome.set(Some(("cancelled", self.inspections.get())));
    }
}

#[expect(clippy::unwrap_used, reason = "test helpers use unwrap for concise failure backtraces")]
fn inspect_completion(future: impl Future<Output = ()> + 'static, poll: bool) -> (Vec<u32>, Option<(&'static str, u32)>) {
    let mut executor = Executor::builder().build();
    let outcome = Rc::new(Cell::new(None));
    let inspections = Rc::new(Cell::new(0));
    let inspected = Rc::clone(&inspections);
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), future)
        .inspect(move |_| {
            inspected.set(inspected.get() + 1);
            Some(inspected.get())
        })
        .register(InspectedCompletion {
            inspections,
            outcome: Rc::clone(&outcome),
        });
    let mut descriptions = Vec::new();
    executor.inspect(1, |value: &u32| descriptions.push(*value)).unwrap();
    if poll {
        let _ = executor.execute_cycle();
    }
    drop(executor);
    (descriptions, outcome.get())
}

#[test]
fn inspection_borrows_the_target_that_receives_success() {
    assert_eq!(inspect_completion(std::future::ready(()), true), (vec![1], Some(("completed", 1))));
}

#[test]
fn inspection_borrows_the_target_that_receives_a_panic() {
    assert_eq!(
        inspect_completion(async { panic!("inspected task panic") }, true),
        (vec![1], Some(("panicked", 1)))
    );
}

#[test]
fn inspection_borrows_the_target_that_receives_cancellation() {
    assert_eq!(
        inspect_completion(std::future::pending::<()>(), false),
        (vec![1], Some(("cancelled", 1)))
    );
}

#[test]
fn ordinary_cycles_do_not_inspect_the_completion_target() {
    let mut executor = Executor::builder().build();
    let outcome = Rc::new(Cell::new(None));
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), std::future::ready(()))
        .inspect(|_| -> Option<()> { panic!("ordinary cycles must not inspect task metadata") })
        .register(InspectedCompletion {
            inspections: Rc::new(Cell::new(0)),
            outcome: Rc::clone(&outcome),
        });
    let _ = executor.execute_cycle();

    assert_eq!(outcome.get(), Some(("completed", 0)));
}

#[test]
fn completion_inspection_can_decline_without_losing_the_result() {
    let mut executor = Executor::builder().build();
    let outcome = Rc::new(Cell::new(None));
    let inspections = Rc::new(Cell::new(0));
    let inspected = Rc::clone(&inspections);
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), std::future::ready(()))
        .inspect(move |_| {
            inspected.set(inspected.get() + 1);
            None::<u32>
        })
        .register(InspectedCompletion {
            inspections,
            outcome: Rc::clone(&outcome),
        });
    let mut descriptions = Vec::new();
    assert_eq!(
        executor.inspect(1, |value: &u32| descriptions.push(*value)).unwrap(),
        InspectionStatus::Complete
    );
    assert!(descriptions.is_empty());
    let _ = executor.execute_cycle();
    assert_eq!(outcome.get(), Some(("completed", 1)));
}

struct InspectionInput {
    converted: Rc<Cell<bool>>,
    shutdown: Option<Shutdown>,
}

impl IntoFuture for InspectionInput {
    type Output = u32;
    type IntoFuture = InspectableFuture;

    fn into_future(self) -> Self::IntoFuture {
        self.converted.set(true);
        if let Some(shutdown) = self.shutdown {
            shutdown.trigger(ShutdownCause::default());
        }
        InspectableFuture(42)
    }
}

struct InspectableFuture(u32);

impl Future for InspectableFuture {
    type Output = u32;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(self.0)
    }
}

#[test]
fn inspection_builder_defers_conversion_and_inspects_the_native_future() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let converted = Rc::new(Cell::new(false));
    let builder = tasks
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            InspectionInput {
                converted: Rc::clone(&converted),
                shutdown: None,
            },
        )
        .inspect(|state| Some((42_u32, state)));

    assert!(!converted.get());
    assert!(!executor.has_tasks());
    builder.register(Completion);
    assert!(converted.get());
    let mut descriptions = Vec::new();
    executor
        .inspect(1, |description: &(u32, TaskState)| descriptions.push(*description))
        .unwrap();
    assert_eq!(descriptions, [(42, TaskState::Ready)]);
    let _ = executor.execute_cycle();
    assert!(!executor.has_tasks());
}

#[test]
fn layered_inspectors_emit_descriptions_in_registration_order() {
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), std::future::ready(42))
        .inspect(|state| Some((1_u8, state)))
        .inspect(|state| Some((2_u8, state)))
        .register(Completion);

    let mut descriptions = Vec::new();
    executor
        .inspect(1, |description: &(u8, TaskState)| descriptions.push(*description))
        .unwrap();
    assert_eq!(descriptions, [(1, TaskState::Ready), (2, TaskState::Ready)]);

    let _ = executor.execute_cycle();
}

#[test]
fn inspected_input_checks_shutdown_before_and_after_conversion() {
    #[derive(Debug)]
    struct Canceled(Rc<Cell<bool>>);

    impl TaskCompletion<u32> for Canceled {
        fn completed(self, _: u32) {
            panic!("a rejected task must not complete");
        }

        fn panicked(self, panic: TaskPanicPayload, _: Option<ShutdownCause>) {
            arty_executor::discard_task_panic_payload(panic);
            panic!("a rejected task must not run");
        }

        fn cancelled(self, _: ShutdownCause) {
            self.0.set(true);
        }
    }

    for stop_during_conversion in [false, true] {
        let executor = Executor::builder().build();
        let tasks = executor.tasks();
        let converted = Rc::new(Cell::new(false));
        let canceled = Rc::new(Cell::new(false));
        let builder = tasks
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                InspectionInput {
                    converted: Rc::clone(&converted),
                    shutdown: stop_during_conversion.then(|| executor.shutdown()),
                },
            )
            .inspect(|_| Some(42_u32));

        if !stop_during_conversion {
            executor.shutdown().trigger(ShutdownCause::default());
        }
        builder.register(Canceled(Rc::clone(&canceled)));

        assert_eq!(converted.get(), stop_during_conversion);
        assert!(canceled.get());
        assert!(!executor.has_tasks());
    }
}

#[test]
fn ordinary_cycles_never_construct_diagnostic_descriptions() {
    let mut executor = Executor::builder().build();
    let calls = Rc::new(Cell::new(0));
    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .inspect({
            let calls = Rc::clone(&calls);
            move |state| {
                calls.set(calls.get() + 1);
                Some(state)
            }
        })
        .register(Completion);

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
        .task(TaskIdentity::from_location(std::panic::Location::caller()), future)
        .inspect(Some)
        .register(Completion);
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
    tasks
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .register(Completion);
    tasks
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .register(Completion);
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
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                std::future::pending::<()>(),
            )
            .inspect(move |_| Some(id))
            .register(Completion);
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
fn inspection_ignores_descriptions_of_a_different_type() {
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .inspect(Some)
        .register(Completion);
    let mut described = Vec::new();

    let status = executor.inspect(1, |value: &u32| described.push(*value)).unwrap();

    assert_eq!((status, described), (InspectionStatus::Complete, Vec::new()));
}

#[test]
fn inspectors_can_decline_to_describe_a_task() {
    let mut executor = Executor::builder().build();
    let calls = Rc::new(Cell::new(0));
    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .inspect({
            let calls = Rc::clone(&calls);
            move |_| {
                calls.set(calls.get() + 1);
                None::<TaskState>
            }
        })
        .register(Completion);
    let mut described = Vec::new();

    let status = executor.inspect(1, |state: &TaskState| described.push(*state)).unwrap();

    assert_eq!((status, described, calls.get()), (InspectionStatus::Complete, Vec::new(), 1));
}

#[test]
fn invalid_limits_and_panicking_visitors_report_errors() {
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .inspect(|_| Some(42_u32))
        .register(Completion);

    let invalid = executor.inspect::<u32>(0, |_| {}).unwrap_err();
    let failed = executor.inspect::<u32>(1, |_| panic!("visitor failed")).unwrap_err();

    assert_eq!((invalid, failed), (InspectionError::InvalidLimit, InspectionError::Panicked));
    assert_eq!(invalid.to_string(), "the task inspection limit must be greater than zero");
    assert_eq!(failed.to_string(), "the task instrumentation or visitor panicked");
    assert_eq!(InspectionError::Busy.to_string(), "native task storage is busy");
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
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), {
            let ran = Rc::clone(&ran);
            async move {
                ran.set(true);
            }
        })
        .inspect(|_| -> Option<u32> { std::panic::panic_any(PanicOnDrop) })
        .register(Completion);
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
        fn poll_finished(&self) {
            panic!("no poll started");
        }
        fn inspection_failed(&self) {
            panic!("observer failures must propagate");
        }
    }
    let mut executor = Executor::builder().build();
    for id in 1..=3 {
        executor
            .tasks()
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                std::future::pending::<()>(),
            )
            .inspect(move |_| Some(id))
            .register(Completion);
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
        fn poll_finished(&self) {
            panic!("no observed poll completed");
        }
        fn inspection_failed(&self) {
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
        .task(TaskIdentity::from_location(std::panic::Location::caller()), future)
        .inspect(|state| Some((7_u32, state)))
        .register(Completion);
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
    struct Payload;
    impl Drop for Payload {
        fn drop(&mut self) {
            panic!("inspector panic payload destructor");
        }
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
            std::panic::panic_any(Payload);
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
            .task(TaskIdentity::from_location(std::panic::Location::caller()), future)
            .inspect({
                let capture = Capture(Rc::clone(&drops));
                move |_| {
                    let _keep = &capture;
                    Some(())
                }
            })
            .register(Report(Rc::clone(&terminal)));

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

    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .inspect(|_| Some(42_u32))
        .register(Canceled(Rc::clone(&canceled)));

    assert!(canceled.get());
}

#[test]
fn rejected_inspector_cleanup_panics_are_disposed_before_cancellation() {
    use std::sync::{Arc, Mutex};

    type Calls = Arc<Mutex<Vec<&'static str>>>;

    #[derive(Debug)]
    struct Canceled(Calls);
    impl TaskCompletion<u32> for Canceled {
        fn completed(self, _: u32) {
            panic!("a rejected task must not complete");
        }
        fn panicked(self, panic: TaskPanicPayload, _: Option<ShutdownCause>) {
            arty_executor::discard_task_panic_payload(panic);
            panic!("a rejected task must not run");
        }
        fn cancelled(self, _: ShutdownCause) {
            self.0.lock().unwrap().push("cancelled");
        }
    }

    struct Payload(Calls);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("payload dropped");
            panic!("diagnostic panic payload destructor");
        }
    }
    struct Capture(Calls);
    impl Drop for Capture {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("capture dropped");
            std::panic::panic_any(Payload(Arc::clone(&self.0)));
        }
    }

    for stop_during_conversion in [false, true] {
        let executor = Executor::builder().build();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let converted = Rc::new(Cell::new(false));
        if !stop_during_conversion {
            executor.shutdown().trigger(ShutdownCause::default());
        }

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor
                .tasks()
                .task(
                    TaskIdentity::from_location(std::panic::Location::caller()),
                    InspectionInput {
                        converted: Rc::clone(&converted),
                        shutdown: stop_during_conversion.then(|| executor.shutdown()),
                    },
                )
                .inspect({
                    let capture = Capture(Arc::clone(&calls));
                    move |_| {
                        let _keep = &capture;
                        Some(())
                    }
                })
                .register(Canceled(Arc::clone(&calls)));
        }));
        let contained = rejected.is_ok();
        if let Err(panic) = rejected {
            arty_executor::discard_task_panic_payload(panic);
        }

        assert_eq!(
            (contained, std::mem::take(&mut *calls.lock().unwrap())),
            (true, vec!["capture dropped", "payload dropped", "cancelled"])
        );
        assert_eq!(converted.get(), stop_during_conversion);
        assert!(!executor.has_tasks());
    }
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

    fn poll_finished(&self) {
        self.finishes.set(self.finishes.get() + 1);
    }

    fn inspection_failed(&self) {
        self.errors.set(self.errors.get() + 1);
    }
}

#[test]
fn observed_polls_pair_callbacks_only_for_described_tasks() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    tasks
        .task(TaskIdentity::from_location(std::panic::Location::caller()), async {})
        .register(Completion);
    tasks
        .task(TaskIdentity::from_location(std::panic::Location::caller()), async {})
        .inspect(|_| Some(42_u32))
        .inspect(|_| Some(42_u32))
        .register(Completion);
    let observer = Observer::default();

    let _ = executor.execute_cycle_observed(&observer);

    assert_eq!((observer.starts.get(), observer.finishes.get(), observer.errors.get()), (2, 2, 0));
}

#[test]
fn observed_polls_borrow_the_completion_target() {
    let mut executor = Executor::builder().build();
    let outcome = Rc::new(Cell::new(None));
    let inspections = Rc::new(Cell::new(41));
    let inspected = Rc::clone(&inspections);
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), std::future::ready(()))
        .inspect(move |state| {
            assert_eq!(state, TaskState::Running);
            inspected.set(inspected.get() + 1);
            Some(inspected.get())
        })
        .register(InspectedCompletion {
            inspections,
            outcome: Rc::clone(&outcome),
        });
    let observer = Observer::default();

    let _ = executor.execute_cycle_observed(&observer);

    assert_eq!((observer.starts.get(), observer.finishes.get(), observer.errors.get()), (1, 1, 0));
    assert_eq!(outcome.get(), Some(("completed", 42)));
}

#[test]
fn rejected_tasks_cancel_the_completion_target_without_inspecting_it() {
    let executor = Executor::builder().build();
    executor.shutdown().trigger(ShutdownCause::default());
    let outcome = Rc::new(Cell::new(None));

    executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            std::future::pending::<()>(),
        )
        .inspect(|_| -> Option<()> { panic!("a stopped executor must not inspect rejected tasks") })
        .register(InspectedCompletion {
            inspections: Rc::new(Cell::new(7)),
            outcome: Rc::clone(&outcome),
        });

    assert_eq!(outcome.get(), Some(("cancelled", 7)));
}

#[test]
fn a_panicking_description_does_not_prevent_its_task_from_running() {
    let mut executor = Executor::builder().build();
    let ran = Rc::new(Cell::new(false));
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), {
            let ran = Rc::clone(&ran);
            async move {
                ran.set(true);
            }
        })
        .inspect(|_| -> Option<u32> {
            panic!("broken inspector");
        })
        .register(Completion);
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
        fn poll_finished(&self) {
            self.0.borrow_mut().push("finished");
        }
        fn inspection_failed(&self) {
            self.0.borrow_mut().push("failed");
        }
    }
    let mut executor = Executor::builder().build();
    executor
        .tasks()
        .task(TaskIdentity::from_location(std::panic::Location::caller()), async {})
        .inspect(|_| Some(Description))
        .register(Completion);
    let observer = Calls::default();

    let _ = executor.execute_cycle_observed(&observer);

    assert_eq!(*observer.0.borrow(), ["started", "failed"]);
}
