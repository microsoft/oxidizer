// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavioral coverage for the unstable Arty driver host boundary.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::any::type_name;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{RawWaker, RawWakerVTable, Wake, Waker};
use std::time::Duration;
use std::{error, fmt};

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use arty_io_host::{DriverHost, IoRuntime};
use observed::{Severity, Value};
use observed_testing::{CapturedEvent, MockProcessor, TEST_ID, test_emitter};
use thread_aware::{Thread, ThreadAware, ThreadBuilder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    First,
    Second,
}

#[derive(Default)]
struct State {
    roles: Mutex<Vec<(Kind, DriverRole)>>,
    relocations: Mutex<Vec<(Kind, Thread)>>,
    driver_threads: Mutex<Vec<(Kind, Thread)>>,
    cycles: Mutex<Vec<(Kind, Duration)>>,
    shutdown_order: Mutex<Vec<Kind>>,
    nested_context_errors: Mutex<Vec<String>>,
    nested_cycle_errors: Mutex<Vec<String>>,
    driver_waker: Mutex<Option<Waker>>,
    panic_shutdown: Mutex<Option<Kind>>,
    // These test-only flags and counters use SeqCst for straightforward
    // observations; channels and joins provide explicit cross-thread phasing.
    context_calls: AtomicUsize,
    format_calls: AtomicUsize,
    shutdowns: AtomicUsize,
    panic_context: AtomicBool,
    panic_initialization: AtomicBool,
    fail_initialization: AtomicBool,
    fail_cycle: AtomicBool,
    reenter_cycle: AtomicBool,
    fail_shutdown: AtomicBool,
    interrupts: AtomicUsize,
}

#[derive(Clone)]
struct Context<const SECOND: bool> {
    state: Arc<State>,
}

impl<const SECOND: bool> ThreadAware for Context<SECOND> {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        assert!(source.is_none());
        let kind = if SECOND { Kind::Second } else { Kind::First };
        self.state.relocations.lock().unwrap().push((kind, destination.clone()));
    }
}

#[derive(Clone)]
struct ReentrantContext {
    state: Arc<State>,
}

impl ThreadAware for ReentrantContext {
    fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
        assert!(source.is_none());
    }
}

impl IoContext for ReentrantContext {
    type ContextOptions = (IoRuntime, Arc<State>);
    type Driver = TestDriver;

    fn context((runtime, state): Self::ContextOptions) -> Self {
        let thread = ThreadBuilder::default().build(std::thread::current().id());
        let host = runtime.driver_host(thread, Waker::noop().clone()).unwrap();
        let _ = host.context::<Context<true>>(Arc::clone(&state)).unwrap();
        Self { state }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        self.state.roles.lock().unwrap().push((Kind::First, options.role()));
        Ok(TestDriver {
            kind: Kind::First,
            state: Arc::clone(&self.state),
        })
    }
}

/// Retrieves its own type through a second host so the runtime-wide reentry
/// guard, rather than the caller host's local guard, owns the rejection.
#[derive(Clone)]
struct SameTypeReentrantContext {
    state: Arc<State>,
}

impl ThreadAware for SameTypeReentrantContext {
    fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
        assert!(source.is_none());
    }
}

impl IoContext for SameTypeReentrantContext {
    type ContextOptions = Arc<State>;
    type Driver = TestDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self { state: options }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        self.state.roles.lock().unwrap().push((Kind::First, options.role()));
        let error = SAME_TYPE_RUNTIME.with_borrow(|runtime| {
            host(runtime.as_ref().unwrap())
                .context::<Self>(Arc::clone(&self.state))
                .err()
                .unwrap()
        });
        self.state.nested_context_errors.lock().unwrap().push(error.to_string());
        Ok(TestDriver {
            kind: Kind::First,
            state: Arc::clone(&self.state),
        })
    }
}

#[derive(Clone)]
struct HandledFailureContext {
    state: Arc<State>,
}

impl ThreadAware for HandledFailureContext {
    fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
        assert!(source.is_none());
    }
}

impl IoContext for HandledFailureContext {
    type ContextOptions = (IoRuntime, Arc<State>);
    type Driver = TestDriver;

    fn context((runtime, state): Self::ContextOptions) -> Self {
        state.fail_initialization.store(true, Ordering::SeqCst);
        let host = host(&runtime);
        let _ = host.context::<Context<true>>(Arc::clone(&state));
        state.fail_initialization.store(false, Ordering::SeqCst);
        Self { state }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        self.state.roles.lock().unwrap().push((Kind::First, options.role()));
        Ok(TestDriver {
            kind: Kind::First,
            state: Arc::clone(&self.state),
        })
    }
}

#[derive(Clone)]
struct FormattingFailureContext {
    state: Arc<State>,
}

impl ThreadAware for FormattingFailureContext {
    fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
        assert!(source.is_none());
    }
}

impl IoContext for FormattingFailureContext {
    type ContextOptions = Arc<State>;
    type Driver = FormattingFailureDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self { state: options }
    }

    fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
        Err(DriverError::from_source(ReentrantFormatError {
            state: Arc::clone(&self.state),
        }))
    }
}

struct FormattingFailureDriver;

impl Driver for FormattingFailureDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        Ok(())
    }
}

struct ReentrantFormatError {
    state: Arc<State>,
}

impl fmt::Debug for ReentrantFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReentrantFormatError").finish_non_exhaustive()
    }
}

impl fmt::Display for ReentrantFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.state.format_calls.fetch_add(1, Ordering::SeqCst);
        FORMAT_RUNTIME.with_borrow(|runtime| {
            let host = host(runtime.as_ref().expect("the formatter test installs its runtime"));
            let _ = host.context::<Context<true>>(Arc::clone(&self.state)).unwrap();
        });
        f.write_str("formatting failed")
    }
}

impl error::Error for ReentrantFormatError {}

thread_local! {
    static FORMAT_RUNTIME: RefCell<Option<IoRuntime>> = const { RefCell::new(None) };
    static SAME_TYPE_RUNTIME: RefCell<Option<IoRuntime>> = const { RefCell::new(None) };
}

impl<const SECOND: bool> IoContext for Context<SECOND> {
    type ContextOptions = Arc<State>;
    type Driver = TestDriver;

    fn context(options: Self::ContextOptions) -> Self {
        options.context_calls.fetch_add(1, Ordering::SeqCst);
        assert!(!options.panic_context.load(Ordering::SeqCst), "context construction failed");
        Self { state: options }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        let kind = if SECOND { Kind::Second } else { Kind::First };
        self.state.roles.lock().unwrap().push((kind, options.role()));
        self.state.driver_threads.lock().unwrap().push((kind, options.thread().clone()));
        assert!(
            !self.state.panic_initialization.load(Ordering::SeqCst),
            "driver initialization panicked"
        );
        if self.state.fail_initialization.load(Ordering::SeqCst) {
            return Err(DriverError::from_message("initialization failed"));
        }
        Ok(TestDriver {
            kind,
            state: Arc::clone(&self.state),
        })
    }
}

/// Intentionally lacks `Clone` to verify that context options are moved.
struct MoveOnlyOptions(Arc<State>);

#[derive(Clone)]
struct MoveOnlyContext {
    state: Arc<State>,
}

impl ThreadAware for MoveOnlyContext {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        assert!(source.is_none());
        self.state.relocations.lock().unwrap().push((Kind::First, destination.clone()));
    }
}

impl IoContext for MoveOnlyContext {
    type ContextOptions = MoveOnlyOptions;
    type Driver = TestDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self { state: options.0 }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        self.state.roles.lock().unwrap().push((Kind::First, options.role()));
        Ok(TestDriver {
            kind: Kind::First,
            state: Arc::clone(&self.state),
        })
    }
}

struct TestDriver {
    kind: Kind,
    state: Arc<State>,
}

impl Driver for TestDriver {
    fn waker(&self) -> Waker {
        self.state
            .driver_waker
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Waker::from(Arc::new(InterruptWake(Arc::clone(&self.state)))))
    }

    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
        if self.state.reenter_cycle.load(Ordering::SeqCst) {
            NESTED_CYCLE_HOST.with_borrow(|host| {
                let host = host
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .expect("the nested cycle test retains its host");
                let error = host.execute_cycle(Duration::ZERO).unwrap_err();
                self.state.nested_cycle_errors.lock().unwrap().push(error.to_string());
            });
        }
        self.state.cycles.lock().unwrap().push((self.kind, cycle.max_wait()));
        if self.state.fail_cycle.load(Ordering::SeqCst) {
            Err(DriverError::from_message("cycle failed"))
        } else {
            Ok(())
        }
    }

    fn shutdown(self) -> Result<(), DriverError> {
        self.state.shutdown_order.lock().unwrap().push(self.kind);
        self.state.shutdowns.fetch_add(1, Ordering::SeqCst);
        assert_ne!(
            *self.state.panic_shutdown.lock().unwrap(),
            Some(self.kind),
            "driver shutdown panicked"
        );
        if self.state.fail_shutdown.load(Ordering::SeqCst) {
            Err(DriverError::from_message("shutdown failed"))
        } else {
            Ok(())
        }
    }
}

thread_local! {
    static NESTED_CYCLE_HOST: RefCell<Option<Weak<DriverHost>>> = const { RefCell::new(None) };
}

struct InterruptWake(Arc<State>);

impl Wake for InterruptWake {
    fn wake(self: Arc<Self>) {
        self.0.interrupts.fetch_add(1, Ordering::SeqCst);
    }
}

struct WakeHostOnDrop {
    host_interrupt: Waker,
    dropped: Arc<AtomicBool>,
}

impl Wake for WakeHostOnDrop {
    fn wake(self: Arc<Self>) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl Drop for WakeHostOnDrop {
    fn drop(&mut self) {
        self.host_interrupt.wake_by_ref();
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct CloneHostWake {
    host_interrupt: Waker,
    cloned: Arc<AtomicBool>,
}

fn clone_host_waker(host_interrupt: Waker, cloned: Arc<AtomicBool>) -> Waker {
    let state = Arc::new(CloneHostWake { host_interrupt, cloned });
    let raw = RawWaker::new(Arc::into_raw(state).cast(), &CLONE_HOST_VTABLE);
    // SAFETY: CLONE_HOST_VTABLE maintains one Arc strong reference for every RawWaker.
    unsafe { Waker::from_raw(raw) }
}

unsafe fn clone_host_waker_raw(data: *const ()) -> RawWaker {
    // SAFETY: data was created by Arc::into_raw in clone_host_waker or this function.
    let state = unsafe { &*data.cast::<CloneHostWake>() };
    state.cloned.store(true, Ordering::SeqCst);
    state.host_interrupt.wake_by_ref();
    // SAFETY: the source RawWaker still owns a strong reference, and the new
    // RawWaker takes ownership of the incremented reference.
    unsafe { Arc::increment_strong_count(data.cast::<CloneHostWake>()) };
    RawWaker::new(data, &CLONE_HOST_VTABLE)
}

unsafe fn wake_host_waker(data: *const ()) {
    // SAFETY: consuming wake owns one Arc strong reference.
    drop(unsafe { Arc::<CloneHostWake>::from_raw(data.cast()) });
}

unsafe fn wake_host_waker_by_ref(_data: *const ()) {}

unsafe fn drop_host_waker(data: *const ()) {
    // SAFETY: dropping the RawWaker releases its owned Arc strong reference.
    drop(unsafe { Arc::<CloneHostWake>::from_raw(data.cast()) });
}

static CLONE_HOST_VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_host_waker_raw, wake_host_waker, wake_host_waker_by_ref, drop_host_waker);

fn host(runtime: &IoRuntime) -> DriverHost {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    runtime.clone().driver_host(thread, Waker::noop().clone()).unwrap()
}

fn test_runtime() -> IoRuntime {
    IoRuntime::new(observed::Sink::noop())
}

fn telemetry_runtime() -> (IoRuntime, MockProcessor) {
    let (sink, processor) = test_emitter(TEST_ID);
    (IoRuntime::new(sink), processor)
}

fn dimension(event: &CapturedEvent, key: &str) -> Option<Value> {
    event.dimensions().into_iter().find(|(name, _)| name == key).map(|(_, value)| value)
}

#[test]
fn first_context_is_primary_and_later_contexts_are_secondary() {
    let runtime = test_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());

    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    let _ = host.context::<Context<true>>(Arc::clone(&state)).unwrap();

    assert_eq!(
        state.roles.lock().unwrap().iter().map(|(_, role)| *role).collect::<Vec<_>>(),
        [DriverRole::Primary, DriverRole::Secondary]
    );
}

#[test]
fn runtime_identity_is_shared_only_by_clones() {
    let runtime = test_runtime();

    assert_eq!(runtime, runtime.clone());
    assert_ne!(runtime, test_runtime());
    assert_eq!(format!("{runtime:?}"), "IoRuntime { .. }");
}

#[test]
fn worker_local_drivers_are_created_lazily() {
    let runtime = test_runtime();
    let first = host(&runtime);
    let second = host(&runtime);
    let state = Arc::new(State::default());

    let _ = first.context::<Context<false>>(Arc::clone(&state)).unwrap();
    assert_eq!(state.roles.lock().unwrap().len(), 1);

    let _ = first.context::<Context<false>>(Arc::clone(&state)).unwrap();
    assert_eq!(state.roles.lock().unwrap().len(), 1);

    let _ = second.context::<Context<false>>(Arc::clone(&state)).unwrap();
    assert_eq!(state.roles.lock().unwrap().len(), 2);
    assert_eq!(state.context_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn context_accepts_move_only_options() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());

    let _ = host.context::<MoveOnlyContext>(MoveOnlyOptions(Arc::clone(&state))).unwrap();

    assert_eq!(state.roles.lock().unwrap().as_slice(), [(Kind::First, DriverRole::Primary)]);
}

#[test]
fn worker_thread_identity_is_used_for_relocation_and_driver_creation() {
    let runtime = test_runtime();
    let state = Arc::new(State::default());
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let worker = std::thread::spawn({
        let state = Arc::clone(&state);
        move || {
            let thread = ThreadBuilder::default().build(std::thread::current().id());
            let host = runtime.driver_host(thread.clone(), Waker::noop().clone()).unwrap();
            let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
            ready_tx.send((host.waker(), thread)).unwrap();
            release_rx.recv().unwrap();
            drop(host);
        }
    });
    let (interrupt, thread) = ready_rx.recv().unwrap();

    interrupt.wake_by_ref();
    assert_eq!(state.interrupts.load(Ordering::SeqCst), 1);
    assert_eq!(state.relocations.lock().unwrap().as_slice(), [(Kind::First, thread.clone())]);
    assert_eq!(state.driver_threads.lock().unwrap().as_slice(), [(Kind::First, thread)]);

    release_tx.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn cycles_run_secondaries_before_primary_with_zero_secondary_wait() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let wait = Duration::from_millis(7);
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    let _ = host.context::<Context<true>>(Arc::clone(&state)).unwrap();

    host.execute_cycle(wait).unwrap();

    let cycles = state.cycles.lock().unwrap();
    assert!(matches!(cycles[0].0, Kind::Second));
    assert_eq!(cycles[0].1, Duration::ZERO);
    assert!(matches!(cycles[1].0, Kind::First));
    assert_eq!(cycles[1].1, wait);
}

#[test]
fn cycle_failures_are_returned_to_the_runtime() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    state.fail_cycle.store(true, Ordering::SeqCst);

    let error = host.execute_cycle(Duration::ZERO).unwrap_err();

    assert_eq!(error.to_string(), "cycle failed");
}

#[test]
fn telemetry_reports_successful_driver_lifecycle_in_order() {
    let (runtime, processor) = telemetry_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    let _ = host.context::<Context<true>>(Arc::clone(&state)).unwrap();

    drop(host);

    let events = processor.events();
    assert_eq!(
        events.iter().map(CapturedEvent::name).collect::<Vec<_>>(),
        [
            "arty.rt.io.driver.initialized",
            "arty.rt.io.driver.initialized",
            "arty.rt.io.driver.stopped",
            "arty.rt.io.driver.stopped",
        ]
    );
    assert_eq!(
        events.iter().map(CapturedEvent::severity).collect::<Vec<_>>(),
        [Some(Severity::Debug); 4]
    );
    assert_eq!(dimension(&events[0], "io.context"), Some(type_name::<Context<false>>().into()));
    assert_eq!(dimension(&events[0], "io.driver.role"), Some("primary".into()));
    assert_eq!(dimension(&events[1], "io.context"), Some(type_name::<Context<true>>().into()));
    assert_eq!(dimension(&events[1], "io.driver.role"), Some("secondary".into()));
    assert_eq!(dimension(&events[2], "io.context"), Some(type_name::<Context<true>>().into()));
    assert_eq!(dimension(&events[2], "io.driver.role"), Some("secondary".into()));
    assert_eq!(dimension(&events[3], "io.context"), Some(type_name::<Context<false>>().into()));
    assert_eq!(dimension(&events[3], "io.driver.role"), Some("primary".into()));
}

#[test]
fn telemetry_reports_failures_with_error_dimensions() {
    let (runtime, processor) = telemetry_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    state.fail_cycle.store(true, Ordering::SeqCst);

    let _ = host.execute_cycle(Duration::ZERO).unwrap_err();
    drop(host);

    let events = processor.events();
    assert_eq!(
        events.iter().map(CapturedEvent::name).collect::<Vec<_>>(),
        [
            "arty.rt.io.driver.initialized",
            "arty.rt.io.driver.cycle_failed",
            "arty.rt.io.driver.stopped",
        ]
    );
    let failed = &events[1];
    assert_eq!(failed.severity(), Some(Severity::Error));
    assert_eq!(dimension(failed, "io.context"), Some(type_name::<Context<false>>().into()));
    assert_eq!(dimension(failed, "io.driver.role"), Some("primary".into()));
    assert_eq!(dimension(failed, "error.message"), Some("cycle failed".into()));
    assert_eq!(dimension(failed, "exception.message"), None);
}

#[test]
fn telemetry_reports_initialization_and_shutdown_failures() {
    let (runtime, initialization_processor) = telemetry_runtime();
    let initialization_host = host(&runtime);
    let state = Arc::new(State::default());
    state.fail_initialization.store(true, Ordering::SeqCst);

    let Err(_error) = initialization_host.context::<Context<false>>(Arc::clone(&state)) else {
        panic!("driver initialization should fail");
    };

    let initialization_events = initialization_processor.events();
    assert_eq!(
        initialization_events.iter().map(CapturedEvent::name).collect::<Vec<_>>(),
        ["arty.rt.io.driver.initialization_failed"]
    );
    assert_eq!(initialization_events[0].severity(), Some(Severity::Error));
    assert_eq!(
        dimension(&initialization_events[0], "error.message"),
        Some("initialization failed".into())
    );

    let (runtime, shutdown_processor) = telemetry_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    state.fail_shutdown.store(true, Ordering::SeqCst);

    drop(host);

    let shutdown_events = shutdown_processor.events();
    assert_eq!(
        shutdown_events.iter().map(CapturedEvent::name).collect::<Vec<_>>(),
        ["arty.rt.io.driver.initialized", "arty.rt.io.driver.shutdown_failed"]
    );
    assert_eq!(shutdown_events[1].severity(), Some(Severity::Error));
    assert_eq!(dimension(&shutdown_events[1], "error.message"), Some("shutdown failed".into()));
}

#[test]
fn nested_cycle_execution_returns_an_error() {
    let host = Rc::new(host(&test_runtime()));
    let state = Arc::new(State::default());
    NESTED_CYCLE_HOST.with_borrow_mut(|installed| *installed = Some(Rc::downgrade(&host)));
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    state.reenter_cycle.store(true, Ordering::SeqCst);

    host.execute_cycle(Duration::ZERO).unwrap();

    assert_eq!(
        *state.nested_cycle_errors.lock().unwrap(),
        ["I/O driver cycle execution cannot be nested"]
    );
    NESTED_CYCLE_HOST.with_borrow_mut(Option::take);
}

#[test]
fn driver_initialization_failures_are_returned_to_the_runtime() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    state.fail_initialization.store(true, Ordering::SeqCst);

    let Err(error) = host.context::<Context<false>>(Arc::clone(&state)) else {
        panic!("driver initialization should fail");
    };

    assert_eq!(error.to_string(), "initialization failed");

    state.fail_initialization.store(false, Ordering::SeqCst);
    let Err(retry_error) = host.context::<Context<false>>(Arc::clone(&state)) else {
        panic!("runtime failure should reject later retrievals");
    };
    assert_eq!(retry_error.to_string(), "initialization failed");
}

#[test]
fn driver_initialization_failure_keeps_existing_drivers_until_host_drop() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    state.fail_initialization.store(true, Ordering::SeqCst);

    let Err(error) = host.context::<Context<true>>(Arc::clone(&state)) else {
        panic!("driver initialization should fail");
    };

    assert_eq!(error.to_string(), "initialization failed");
    assert!(state.shutdown_order.lock().unwrap().is_empty());
    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 0);
    drop(host);
    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn panicking_driver_initialization_keeps_the_registered_context() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    state.panic_initialization.store(true, Ordering::SeqCst);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = host.context::<Context<false>>(Arc::clone(&state));
    }));
    assert!(panicked.is_err());

    state.panic_initialization.store(false, Ordering::SeqCst);
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();

    assert_eq!(state.context_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.roles.lock().unwrap().iter().map(|(_, role)| *role).collect::<Vec<_>>(),
        [DriverRole::Primary, DriverRole::Primary]
    );
}

#[test]
fn runtime_failure_rejects_cached_context_retrievals() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    state.fail_initialization.store(true, Ordering::SeqCst);
    let Err(error) = host.context::<Context<true>>(Arc::clone(&state)) else {
        panic!("driver initialization should fail");
    };
    assert_eq!(error.to_string(), "initialization failed");

    state.fail_initialization.store(false, Ordering::SeqCst);
    let Err(cached_error) = host.context::<Context<false>>(Arc::clone(&state)) else {
        panic!("runtime failure should reject cached retrievals");
    };
    assert_eq!(cached_error.to_string(), "initialization failed");
}

#[test]
fn failure_formatting_can_reenter_context_retrieval() {
    let runtime = test_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());
    FORMAT_RUNTIME.with_borrow_mut(|installed| *installed = Some(runtime));

    let Err(_error) = host.context::<FormattingFailureContext>(Arc::clone(&state)) else {
        panic!("driver initialization should fail");
    };
    FORMAT_RUNTIME.with_borrow_mut(Option::take);

    assert_eq!(state.format_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.roles.lock().unwrap().last().unwrap().1, DriverRole::Secondary);
    let Err(stored_error) = host.context::<Context<false>>(Arc::clone(&state)) else {
        panic!("runtime failure should reject later retrievals");
    };
    assert_eq!(stored_error.to_string(), "formatting failed");
}

#[test]
fn context_construction_panics_do_not_poison_the_registry() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    state.panic_context.store(true, Ordering::SeqCst);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = host.context::<Context<false>>(Arc::clone(&state));
    }));
    assert!(result.is_err());

    state.panic_context.store(false, Ordering::SeqCst);
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    assert_eq!(state.roles.lock().unwrap().last().unwrap().1, DriverRole::Primary);
}

#[test]
fn context_construction_can_reenter_the_runtime_registry() {
    let runtime = test_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());

    let _ = host.context::<ReentrantContext>((runtime, Arc::clone(&state))).unwrap();

    assert_eq!(
        state.roles.lock().unwrap().iter().map(|(_, role)| *role).collect::<Vec<_>>(),
        [DriverRole::Secondary, DriverRole::Primary]
    );
}

#[test]
fn same_type_retrieval_through_another_host_rejects_reentrant_initialization() {
    let runtime = test_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());
    SAME_TYPE_RUNTIME.with_borrow_mut(|installed| *installed = Some(runtime));

    let _ = host.context::<SameTypeReentrantContext>(Arc::clone(&state)).unwrap();
    SAME_TYPE_RUNTIME.with_borrow_mut(Option::take);

    assert_eq!(
        state.nested_context_errors.lock().unwrap().as_slice(),
        ["I/O context retrieval cannot reenter initialization of the same context type on one thread"]
    );
}

#[test]
fn handled_inner_failure_rejects_outer_retrieval_and_keeps_driver_until_drop() {
    let runtime = test_runtime();
    let host = host(&runtime);
    let state = Arc::new(State::default());

    let Err(error) = host.context::<HandledFailureContext>((runtime, Arc::clone(&state))) else {
        panic!("the outer retrieval should observe the runtime failure");
    };
    assert_eq!(error.to_string(), "initialization failed");
    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 0);

    drop(host);

    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn empty_host_cycle_is_a_noop() {
    let host = host(&test_runtime());

    host.execute_cycle(Duration::from_millis(1)).unwrap();
}

#[test]
fn interrupt_waker_tracks_primary_and_is_safe_after_drop() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let interrupt = host.waker();
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    assert!(format!("{host:?}").starts_with("DriverHost { thread: "));

    interrupt.wake_by_ref();
    assert_eq!(state.interrupts.load(Ordering::SeqCst), 1);

    let owned_interrupt = interrupt.clone();
    owned_interrupt.wake();
    assert_eq!(state.interrupts.load(Ordering::SeqCst), 2);

    drop(host);
    interrupt.wake();
    assert_eq!(state.interrupts.load(Ordering::SeqCst), 2);
}

#[test]
fn dropping_the_primary_waker_can_reenter_the_host_interrupt() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let dropped = Arc::new(AtomicBool::new(false));
    let driver_waker = Waker::from(Arc::new(WakeHostOnDrop {
        host_interrupt: host.waker(),
        dropped: Arc::clone(&dropped),
    }));
    *state.driver_waker.lock().unwrap() = Some(driver_waker);
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();

    drop(host);

    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn waking_the_primary_does_not_clone_the_provider_waker() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let cloned = Arc::new(AtomicBool::new(false));
    let driver_waker = clone_host_waker(host.waker(), Arc::clone(&cloned));
    *state.driver_waker.lock().unwrap() = Some(driver_waker);
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();

    host.waker().wake_by_ref();

    assert!(!cloned.load(Ordering::SeqCst));
}

#[test]
fn drop_attempts_every_driver_after_shutdown_errors() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    state.fail_shutdown.store(true, Ordering::SeqCst);
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    let _ = host.context::<Context<true>>(Arc::clone(&state)).unwrap();

    drop(host);

    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 2);
    assert_eq!(state.shutdown_order.lock().unwrap().as_slice(), [Kind::Second, Kind::First]);
}

#[test]
fn shutdown_panics_propagate_without_skipping_the_panicking_driver() {
    let host = host(&test_runtime());
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(Arc::clone(&state)).unwrap();
    let _ = host.context::<Context<true>>(Arc::clone(&state)).unwrap();
    *state.panic_shutdown.lock().unwrap() = Some(Kind::Second);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(host)));

    assert!(panicked.is_err());
    assert_eq!(state.shutdown_order.lock().unwrap().as_slice(), [Kind::Second]);
}
