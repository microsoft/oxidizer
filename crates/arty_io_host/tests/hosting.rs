// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavioral coverage for the unstable Arty driver host boundary.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{RawWaker, RawWakerVTable, Wake, Waker};
use std::time::Duration;

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use arty_io_host::{DriverHost, IoRuntime};
use thread_aware::{Thread, ThreadAware, ThreadBuilder};

#[derive(Clone, Copy)]
enum Kind {
    First,
    Second,
}

#[derive(Default)]
struct State {
    roles: Mutex<Vec<(Kind, DriverRole)>>,
    cycles: Mutex<Vec<(Kind, Duration)>>,
    driver_waker: Mutex<Option<Waker>>,
    shutdowns: AtomicUsize,
    fail_cycle: AtomicBool,
    fail_shutdown: AtomicBool,
    interrupts: AtomicUsize,
}

#[derive(Clone)]
struct Context<const SECOND: bool> {
    state: Arc<State>,
}

impl<const SECOND: bool> ThreadAware for Context<SECOND> {
    fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
        assert!(source.is_none());
    }
}

impl<const SECOND: bool> IoContext for Context<SECOND> {
    type ContextOptions = Arc<State>;
    type Driver = TestDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self { state: options }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        let kind = if SECOND { Kind::Second } else { Kind::First };
        self.state.roles.lock().unwrap().push((kind, options.role()));
        Ok(TestDriver {
            kind,
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
        self.state.cycles.lock().unwrap().push((self.kind, cycle.max_wait()));
        if self.state.fail_cycle.load(Ordering::SeqCst) {
            Err(DriverError::from_message("cycle failed"))
        } else {
            Ok(())
        }
    }

    fn shutdown(self) -> Result<(), DriverError> {
        self.state.shutdowns.fetch_add(1, Ordering::SeqCst);
        if self.state.fail_shutdown.load(Ordering::SeqCst) {
            Err(DriverError::from_message("shutdown failed"))
        } else {
            Ok(())
        }
    }
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

#[expect(clippy::manual_noop_waker, reason = "custom Wake drop behavior is the contract under test")]
impl Wake for WakeHostOnDrop {
    fn wake(self: Arc<Self>) {}
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
    let mut runtime = runtime.clone();
    runtime.relocate(None, &thread);
    runtime.driver_host(Waker::noop().clone())
}

#[test]
fn first_context_is_primary_and_later_contexts_are_secondary() {
    let runtime = IoRuntime::new(observed::Sink::noop());
    let host = host(&runtime);
    let state = Arc::new(State::default());

    let _ = host.context::<Context<false>>(&state).unwrap();
    let _ = host.context::<Context<true>>(&state).unwrap();

    assert_eq!(
        state.roles.lock().unwrap().iter().map(|(_, role)| *role).collect::<Vec<_>>(),
        [DriverRole::Primary, DriverRole::Secondary]
    );
}

#[test]
fn runtime_identity_is_shared_only_by_clones() {
    let runtime = IoRuntime::new(observed::Sink::noop());

    assert_eq!(runtime, runtime.clone());
    assert_ne!(runtime, IoRuntime::new(observed::Sink::noop()));
}

#[test]
fn worker_local_drivers_are_created_lazily() {
    let runtime = IoRuntime::new(observed::Sink::noop());
    let first = host(&runtime);
    let second = host(&runtime);
    let state = Arc::new(State::default());

    let _ = first.context::<Context<false>>(&state).unwrap();
    assert_eq!(state.roles.lock().unwrap().len(), 1);

    let _ = first.context::<Context<false>>(&state).unwrap();
    assert_eq!(state.roles.lock().unwrap().len(), 1);

    let _ = second.context::<Context<false>>(&state).unwrap();
    assert_eq!(state.roles.lock().unwrap().len(), 2);
}

#[test]
fn cycles_run_secondaries_before_primary_with_zero_secondary_wait() {
    let host = host(&IoRuntime::new(observed::Sink::noop()));
    let state = Arc::new(State::default());
    let wait = Duration::from_millis(7);
    let _ = host.context::<Context<false>>(&state).unwrap();
    let _ = host.context::<Context<true>>(&state).unwrap();

    host.execute_cycle(wait).unwrap();

    let cycles = state.cycles.lock().unwrap();
    assert!(matches!(cycles[0].0, Kind::Second));
    assert_eq!(cycles[0].1, Duration::ZERO);
    assert!(matches!(cycles[1].0, Kind::First));
    assert_eq!(cycles[1].1, wait);
}

#[test]
fn cycle_failures_are_returned_to_the_runtime() {
    let host = host(&IoRuntime::new(observed::Sink::noop()));
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(&state).unwrap();
    state.fail_cycle.store(true, Ordering::SeqCst);

    let error = host.execute_cycle(Duration::ZERO).unwrap_err();

    assert_eq!(error.to_string(), "cycle failed");
}

#[test]
fn interrupt_waker_tracks_primary_and_is_safe_after_drop() {
    let host = host(&IoRuntime::new(observed::Sink::noop()));
    let state = Arc::new(State::default());
    let interrupt = host.waker();
    let _ = host.context::<Context<false>>(&state).unwrap();

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
    let host = host(&IoRuntime::new(observed::Sink::noop()));
    let state = Arc::new(State::default());
    let dropped = Arc::new(AtomicBool::new(false));
    let driver_waker = Waker::from(Arc::new(WakeHostOnDrop {
        host_interrupt: host.waker(),
        dropped: Arc::clone(&dropped),
    }));
    *state.driver_waker.lock().unwrap() = Some(driver_waker);
    let _ = host.context::<Context<false>>(&state).unwrap();

    drop(host);

    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn waking_the_primary_does_not_clone_the_provider_waker() {
    let host = host(&IoRuntime::new(observed::Sink::noop()));
    let state = Arc::new(State::default());
    let cloned = Arc::new(AtomicBool::new(false));
    let driver_waker = clone_host_waker(host.waker(), Arc::clone(&cloned));
    *state.driver_waker.lock().unwrap() = Some(driver_waker);
    let _ = host.context::<Context<false>>(&state).unwrap();

    host.waker().wake_by_ref();

    assert!(!cloned.load(Ordering::SeqCst));
}

#[test]
fn drop_attempts_every_driver_after_shutdown_errors() {
    let host = host(&IoRuntime::new(observed::Sink::noop()));
    let state = Arc::new(State::default());
    state.fail_shutdown.store(true, Ordering::SeqCst);
    let _ = host.context::<Context<false>>(&state).unwrap();
    let _ = host.context::<Context<true>>(&state).unwrap();

    drop(host);

    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 2);
}
