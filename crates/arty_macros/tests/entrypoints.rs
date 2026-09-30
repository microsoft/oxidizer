// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises the public proc-macro shims without a dependency cycle back to Arty.

mod fixture {
    use std::cell::Cell;
    use std::num::NonZero;

    use tick::{Clock, ClockControl};

    thread_local! {
        pub(super) static FAIL_CONSTRUCTION: Cell<bool> = const { Cell::new(false) };
        pub(super) static BUILDER_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) mod __private {
        pub(crate) use tick::ClockControl;

        pub(crate) fn resume_join_error(payload: Box<dyn std::any::Any + Send + 'static>) -> ! {
            std::panic::resume_unwind(payload)
        }
    }

    #[derive(Debug)]
    pub(super) struct Builtins(pub(super) usize, pub(super) usize, pub(super) Option<Clock>);

    #[derive(Debug)]
    pub(super) struct ProcessorCount(NonZero<usize>);

    impl ProcessorCount {
        pub(super) const fn at_most(count: NonZero<usize>) -> Self {
            Self(count)
        }
    }

    #[derive(Debug)]
    pub(super) struct Runtime {
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

        pub(super) fn run<F, Fut, R>(self, factory: F) -> std::thread::Result<R>
        where
            F: FnOnce(Builtins) -> Fut,
            Fut: Future<Output = R>,
        {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                futures::executor::block_on(factory(Builtins(self.value, self.workers, self.clock)))
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

        pub(super) fn processor_count(mut self, count: ProcessorCount) -> Self {
            self.workers = count.0.get().min(2);
            self
        }

        pub(super) fn clock(mut self, control: ClockControl) -> Self {
            self.clock = Some(control.to_clock());
            self
        }

        pub(super) fn build(self) -> Result<Runtime, &'static str> {
            if FAIL_CONSTRUCTION.replace(false) {
                Err("forced construction failure")
            } else {
                Ok(Runtime {
                    value: self.value,
                    workers: self.workers,
                    clock: self.clock,
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
    assert_eq!(entrypoint(), 42);
    runtime_test();
    assert_eq!(qualified_main(), 41);
    qualified_test();
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

#[arty_macros::test(runtime_path = crate::renamed)]
async fn clock_injection_preserves_defaults(cx: fixture::Builtins, control: ClockControl) {
    assert_eq!(cx.1, 2);
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
