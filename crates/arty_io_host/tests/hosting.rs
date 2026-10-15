// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavioral coverage for the unstable Arty driver host boundary.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
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
        Waker::from(Arc::new(InterruptWake(Arc::clone(&self.state))))
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

fn host(runtime: IoRuntime) -> DriverHost {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    DriverHost::new(runtime, thread, Waker::noop().clone(), observed::Sink::noop())
}

#[test]
fn first_context_is_primary_and_later_contexts_are_secondary() {
    let runtime = IoRuntime::new();
    let host = host(runtime);
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
    let runtime = IoRuntime::new();

    assert_eq!(runtime, runtime.clone());
    assert_ne!(runtime, IoRuntime::new());
}

#[test]
fn worker_local_drivers_are_created_lazily() {
    let runtime = IoRuntime::new();
    let first = host(runtime.clone());
    let second = host(runtime);
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
    let host = host(IoRuntime::new());
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
    let host = host(IoRuntime::new());
    let state = Arc::new(State::default());
    let _ = host.context::<Context<false>>(&state).unwrap();
    state.fail_cycle.store(true, Ordering::SeqCst);

    let error = host.execute_cycle(Duration::ZERO).unwrap_err();

    assert_eq!(error.to_string(), "cycle failed");
}

#[test]
fn interrupt_waker_tracks_primary_and_is_safe_after_drop() {
    let host = host(IoRuntime::new());
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
fn drop_attempts_every_driver_after_shutdown_errors() {
    let host = host(IoRuntime::new());
    let state = Arc::new(State::default());
    state.fail_shutdown.store(true, Ordering::SeqCst);
    let _ = host.context::<Context<false>>(&state).unwrap();
    let _ = host.context::<Context<true>>(&state).unwrap();

    drop(host);

    assert_eq!(state.shutdowns.load(Ordering::SeqCst), 2);
}
