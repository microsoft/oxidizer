// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises the public proc-macro shims without a dependency cycle back to Arty.

mod fixture {
    use std::cell::Cell;

    use tick::{Clock, ClockControl};

    thread_local! {
        pub(super) static FAIL_CONSTRUCTION: Cell<bool> = const { Cell::new(false) };
        pub(super) static BUILDER_CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static ENTRYPOINT_BODY_RAN: Cell<bool> = const { Cell::new(false) };
        pub(super) static STOP_CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static FAIL_SHUTDOWN: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) mod __private {
        pub(crate) use tick::ClockControl;

        pub(crate) fn resume_error(payload: Box<dyn std::any::Any + Send + 'static>) -> ! {
            std::panic::resume_unwind(payload)
        }
    }

    #[derive(Debug)]
    pub(super) struct Builtins(pub(super) usize, pub(super) usize, pub(super) Option<Clock>);

    #[derive(Debug, Clone, Copy)]
    pub(super) struct WorkersPolicy(usize);

    impl WorkersPolicy {
        pub(super) const fn at_most(count: usize) -> Self {
            Self(count)
        }
    }

    #[derive(Debug)]
    pub(super) struct Runtime {
        scheduler: RuntimeScheduler,
    }

    #[derive(Debug)]
    pub(super) struct RuntimeScheduler {
        value: usize,
        workers: usize,
        clock: Option<Clock>,
    }

    impl Runtime {
        pub(super) fn new() -> Result<Self, &'static str> {
            Self::builder().build()
        }

        pub(super) const fn builder() -> RuntimeBuilder {
            RuntimeBuilder {
                value: 41,
                workers: 2,
                clock: None,
            }
        }

        pub(super) fn scheduler(&self) -> &RuntimeScheduler {
            &self.scheduler
        }

        pub(super) fn stop(self) -> std::thread::Result<()> {
            STOP_CALLS.set(STOP_CALLS.get().saturating_add(1));
            drop(self);
            if FAIL_SHUTDOWN.replace(false) {
                Err(Box::new("forced shutdown failure"))
            } else {
                Ok(())
            }
        }
    }

    impl RuntimeScheduler {
        pub(super) fn block_on<F, Fut, R>(&self, factory: F) -> std::thread::Result<R>
        where
            F: FnOnce(Builtins) -> Fut,
            Fut: Future<Output = R>,
        {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                futures::executor::block_on(factory(Builtins(self.value, self.workers, self.clock.clone())))
            }))
        }
    }

    #[derive(Debug)]
    pub(super) struct RuntimeBuilder {
        value: usize,
        workers: usize,
        clock: Option<Clock>,
    }

    impl RuntimeBuilder {
        pub(super) const fn value(mut self, value: usize) -> Self {
            self.value = value;
            self
        }

        pub(super) fn workers(mut self, count: WorkersPolicy) -> Self {
            self.workers = count.0.min(2);
            self
        }

        pub(super) fn clock(mut self, control: ClockControl) -> Self {
            self.clock = Some(control.into());
            self
        }

        pub(super) fn build(self) -> Result<Runtime, &'static str> {
            if FAIL_CONSTRUCTION.replace(false) {
                Err("forced construction failure")
            } else if self.workers == 0 {
                Err("worker count must be greater than zero")
            } else {
                Ok(Runtime {
                    scheduler: RuntimeScheduler {
                        value: self.value,
                        workers: self.workers,
                        clock: self.clock,
                    },
                })
            }
        }
    }
}

use fixture as renamed;
use tick::ClockControl;

#[arty_macros::main(runtime_path = crate::fixture)]
async fn entrypoint(cx: fixture::Builtins) -> usize {
    cx.0.saturating_add(1)
}

#[arty_macros::test(runtime_path = crate::fixture)]
async fn runtime_test(cx: fixture::Builtins) {
    assert_eq!(cx.0, 41);
    assert_eq!(cx.1, 1);
}

trait HasContext {
    type Context;
    type Control;
}

struct App;

impl HasContext for App {
    type Context = fixture::Builtins;
    type Control = ClockControl;
}

#[arty_macros::main(runtime_path = crate::fixture)]
async fn qualified_main(cx: <App as HasContext>::Context) -> usize {
    cx.0
}

#[arty_macros::test(runtime_path = crate::fixture)]
async fn qualified_test(cx: <App as HasContext>::Context) {
    assert_eq!(cx.0, 41);
}

#[test]
fn expanded_entry_points_exist_and_execute() {
    fixture::STOP_CALLS.set(0);
    assert_eq!(entrypoint(), 42);
    runtime_test();
    assert_eq!(qualified_main(), 41);
    qualified_test();
    assert_eq!(fixture::STOP_CALLS.get(), 4);
}

#[test]
#[should_panic(expected = "forced construction failure")]
fn constructor_failure_is_not_discarded() {
    fixture::FAIL_CONSTRUCTION.set(true);
    let _ = entrypoint();
}

#[arty_macros::main(workers = 4, runtime_path = crate::renamed)]
async fn limited_workers(cx: fixture::Builtins) -> usize {
    cx.1
}

#[arty_macros::test(workers = 1, runtime_path = crate::renamed)]
async fn one_worker(cx: fixture::Builtins) {
    assert_eq!(cx.1, 1);
}

#[arty_macros::main(workers = 0, runtime_path = crate::renamed)]
async fn zero_worker_entrypoint(_cx: fixture::Builtins) {
    fixture::ENTRYPOINT_BODY_RAN.set(true);
}

#[test]
#[should_panic(expected = "worker count must be greater than zero")]
fn zero_worker_count_is_validated_by_runtime_construction() {
    fixture::ENTRYPOINT_BODY_RAN.set(false);
    let error = std::panic::catch_unwind(zero_worker_entrypoint).unwrap_err();
    assert!(!fixture::ENTRYPOINT_BODY_RAN.get());
    std::panic::resume_unwind(error);
}

type AppResult = Result<usize, &'static str>;

fn custom_builder() -> Result<fixture::RuntimeBuilder, &'static str> {
    fixture::BUILDER_CALLS.set(fixture::BUILDER_CALLS.get() + 1);
    if fixture::FAIL_CONSTRUCTION.replace(false) {
        Err("configuration failure")
    } else {
        Ok(fixture::Runtime::builder().value(17))
    }
}

#[arty_macros::main(builder = custom_builder()?, runtime_path = crate::renamed)]
async fn configured_entrypoint(cx: <App as HasContext>::Context) -> AppResult {
    Ok(cx.0)
}

#[arty_macros::test(builder = fixture::Runtime::builder().value(7), runtime_path = crate::renamed)]
async fn configured_test(cx: fixture::Builtins) {
    assert_eq!(cx.0, 7);
    assert_eq!(cx.1, 2);
    assert!(cx.2.is_none());
}

#[test]
fn configuration_runs_once_on_the_calling_thread() {
    fixture::BUILDER_CALLS.set(0);
    assert_eq!(configured_entrypoint(), Ok(17));
    assert_eq!(fixture::BUILDER_CALLS.get(), 1);
    assert_eq!(limited_workers(), 2);
}

#[test]
fn explicit_configuration_error_preserves_the_result_alias() {
    fixture::FAIL_CONSTRUCTION.set(true);
    assert_eq!(configured_entrypoint(), Err("configuration failure"));
}

#[test]
#[should_panic(expected = "forced construction failure")]
fn configured_constructor_failure_is_not_discarded() {
    fixture::FAIL_CONSTRUCTION.set(true);
    configured_test();
}

#[arty_macros::test(runtime_path = crate::renamed)]
#[should_panic(expected = "original payload")]
async fn original_panic_payload_is_preserved(cx: fixture::Builtins) {
    assert_eq!(cx.0, 41);
    panic!("original payload");
}

#[test]
fn root_panic_stops_the_owner_before_resuming_its_payload() {
    fixture::STOP_CALLS.set(0);
    let payload = std::panic::catch_unwind(original_panic_payload_is_preserved).unwrap_err();
    assert_eq!(fixture::STOP_CALLS.get(), 1);
    assert_eq!(*payload.downcast::<&'static str>().unwrap(), "original payload");
}

#[test]
#[should_panic(expected = "forced shutdown failure")]
fn shutdown_failure_is_reported_after_a_successful_root() {
    fixture::FAIL_SHUTDOWN.set(true);
    let _ = entrypoint();
}

#[test]
fn root_panic_remains_primary_when_shutdown_also_fails() {
    fixture::STOP_CALLS.set(0);
    fixture::FAIL_SHUTDOWN.set(true);
    let payload = std::panic::catch_unwind(original_panic_payload_is_preserved).unwrap_err();
    assert_eq!(fixture::STOP_CALLS.get(), 1);
    assert_eq!(*payload.downcast::<&'static str>().unwrap(), "original payload");
}

#[arty_macros::test(runtime_path = crate::renamed)]
async fn clock_injection_preserves_defaults(cx: fixture::Builtins, control: ClockControl) {
    assert_eq!(cx.1, 1);
    let clock = cx.2.unwrap();
    assert_eq!(clock.system_time(), std::time::UNIX_EPOCH);
    control.advance(std::time::Duration::from_secs(3));
    assert_eq!(clock.system_time(), std::time::UNIX_EPOCH + std::time::Duration::from_secs(3));
}

#[arty_macros::test(workers = 1, runtime_path = crate::renamed)]
async fn generated_names_do_not_shadow_arguments(
    __arty_clock_control: <App as HasContext>::Context,
    mut __arty_builder: <App as HasContext>::Control,
) {
    let __arty_clock_control = __arty_clock_control.2.unwrap();
    __arty_builder = __arty_builder.auto_advance_timers(true);
    __arty_clock_control.delay(std::time::Duration::from_secs(5)).await;
    assert_eq!(__arty_builder.to_clock().system_time(), __arty_clock_control.system_time());
}

#[arty_macros::test(runtime_path = crate::renamed)]
async fn generated_runtime_bindings_do_not_shadow_arguments(
    __arty_runtime: <App as HasContext>::Context,
    __arty_result: <App as HasContext>::Control,
) {
    assert_eq!(__arty_runtime.0, 41);
    assert_eq!(__arty_result.to_clock().system_time(), std::time::UNIX_EPOCH);
}

#[arty_macros::test(
    runtime_path = crate::renamed,
    builder = fixture::Runtime::builder()
        .clock(ClockControl::new_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(123)))
)]
async fn builder_clock_is_not_replaced(cx: fixture::Builtins) {
    assert_eq!(
        cx.2.unwrap().system_time(),
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(123),
    );
}
