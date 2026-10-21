// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::task::Waker;
use std::time::Duration;

use arty_io_core::IoContext;
use arty_io_host::{DriverHost, IoRuntime};
use thread_aware::Thread;

use crate::io::fail_runtime;
use crate::runtime::Error;
use crate::runtime::dispatch::DispatcherClient;

thread_local! {
    static CURRENT: RefCell<Option<RuntimeDriverHost>> = const { RefCell::new(None) };
}

#[derive(Debug)]
pub(crate) struct RuntimeDriverHost {
    runtime: IoRuntime,
    dispatcher: DispatcherClient,
    thread: Thread,
    host: DriverHost,
}

impl RuntimeDriverHost {
    pub(crate) fn install(runtime: &IoRuntime, dispatcher: DispatcherClient, thread: &Thread, owner_waker: Waker) -> Waker {
        let driver_host = runtime.clone().driver_host(owner_waker);
        let waker = driver_host.waker();
        let host = Self {
            runtime: runtime.to_owned(),
            dispatcher,
            thread: thread.to_owned(),
            host: driver_host,
        };
        CURRENT.with_borrow_mut(|current| {
            assert!(current.is_none(), "an I/O driver host is already installed on this worker");
            *current = Some(host);
        });
        waker
    }

    pub(crate) fn uninstall() {
        let host = CURRENT.with_borrow_mut(Option::take);
        drop(host.expect("the worker must retain its I/O driver host until shutdown"));
    }

    pub(crate) fn has_primary_current() -> bool {
        CURRENT.with_borrow(|current| {
            current
                .as_ref()
                .expect("the worker must install its I/O driver host before running")
                .host
                .has_primary()
        })
    }

    pub(crate) fn try_context_current<C>(runtime: &IoRuntime, thread: &Thread, options: &C::ContextOptions) -> Option<Result<C, Error>>
    where
        C: IoContext,
        C::ContextOptions: Clone,
    {
        Self::with_current(runtime, thread, |host| {
            let context = host
                .host
                .context::<C>(options)
                .map_err(|error| fail_runtime(&host.dispatcher, error))?;
            crate::io::runtime_error(&host.dispatcher).map_or(Ok(context), Err)
        })
    }

    fn with_current<R>(runtime: &IoRuntime, thread: &Thread, f: impl FnOnce(&Self) -> R) -> Option<R> {
        CURRENT.with_borrow(|current| {
            current
                .as_ref()
                .filter(|host| host.runtime == *runtime && host.thread == *thread)
                .map(f)
        })
    }

    pub(crate) fn execute_cycle_current(max_wait: Duration) -> Result<(), Error> {
        CURRENT.with_borrow(|current| {
            let host = current
                .as_ref()
                .expect("the worker must install its I/O driver host before running");
            host.host
                .execute_cycle(max_wait)
                .map_err(|error| fail_runtime(&host.dispatcher, error))
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::error::Error as _;

    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions};
    use thread_aware::{ThreadAware, ThreadBuilder};

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TestContext {
        fail_cycle: bool,
    }

    impl ThreadAware for TestContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for TestContext {
        type ContextOptions = bool;
        type Driver = TestDriver;

        fn context(options: Self::ContextOptions) -> Self {
            Self { fail_cycle: options }
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            Ok(TestDriver {
                fail_cycle: self.fail_cycle,
            })
        }
    }

    struct TestDriver {
        fail_cycle: bool,
    }

    impl Driver for TestDriver {
        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
            if self.fail_cycle {
                return Err(DriverError::from_message("cycle failed"));
            }
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    struct InstalledHost;

    impl Drop for InstalledHost {
        fn drop(&mut self) {
            RuntimeDriverHost::uninstall();
        }
    }

    fn host() -> (IoRuntime, Thread, InstalledHost) {
        let thread = ThreadBuilder::default().build(std::thread::current().id());
        let mut runtime = IoRuntime::new(observed::Sink::noop());
        runtime.relocate(None, &thread);
        let dispatcher = DispatcherClient::for_test(&thread);
        let _ = RuntimeDriverHost::install(&runtime, dispatcher, &thread, Waker::noop().clone());
        (runtime, thread, InstalledHost)
    }

    #[test]
    fn current_host_requires_matching_runtime_and_thread() {
        let (runtime, thread, _host) = host();

        assert!(RuntimeDriverHost::with_current(&runtime, &thread, |_| ()).is_some());

        let other_runtime = IoRuntime::new(observed::Sink::noop());
        assert!(RuntimeDriverHost::with_current(&other_runtime, &thread, |_| ()).is_none());

        let other_thread = ThreadBuilder::default().build(std::thread::current().id());
        assert!(RuntimeDriverHost::with_current(&runtime, &other_thread, |_| ()).is_none());
    }

    #[test]
    fn current_context_is_returned_from_the_matching_host() {
        let (runtime, thread, _host) = host();

        let context = RuntimeDriverHost::try_context_current::<TestContext>(&runtime, &thread, &false)
            .unwrap()
            .unwrap();

        assert_eq!(context, TestContext { fail_cycle: false });
    }

    #[test]
    fn primary_driver_state_changes_after_context_initialization() {
        let (runtime, thread, _host) = host();
        let before_initialization = RuntimeDriverHost::has_primary_current();

        RuntimeDriverHost::try_context_current::<TestContext>(&runtime, &thread, &false)
            .unwrap()
            .unwrap();

        assert_eq!((before_initialization, RuntimeDriverHost::has_primary_current()), (false, true));
    }

    #[test]
    fn driver_cycle_failures_are_returned() {
        let (runtime, thread, _host) = host();
        RuntimeDriverHost::try_context_current::<TestContext>(&runtime, &thread, &true)
            .unwrap()
            .unwrap();

        let error = RuntimeDriverHost::execute_cycle_current(Duration::ZERO).unwrap_err();

        assert_eq!(error.source().unwrap().to_string(), "cycle failed");
    }
}
