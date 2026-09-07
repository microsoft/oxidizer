// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

use std::cell::Cell;
use std::fmt;
use std::pin::pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::{Driver, DriverContext, DriverInit, DriverProvider, Parker, SystemTask, SystemTaskSpawner};
use static_assertions::{assert_impl_all, assert_not_impl_any, assert_obj_safe};
use thread_aware_core::{Thread, ThreadAware};

assert_obj_safe!(Parker);
assert_obj_safe!(SystemTaskSpawner);
assert_impl_all!(DriverInit: Clone, Send, Sync, fmt::Debug);

#[test]
fn public_traits_have_expected_object_safety() {
    let parker = TestParker::default();
    let system_tasks = InlineSystemTasks::default();

    let _: &dyn Parker = &parker;
    let _: &dyn SystemTaskSpawner = &system_tasks;
}

#[test]
fn driver_can_remain_thread_local() {
    assert_not_impl_any!(LocalDriver: Send, Sync);

    fn assert_driver<T: Driver>() {}
    assert_driver::<LocalDriver>();
}

#[test]
fn driver_init_exposes_runtime_facilities() {
    let system_tasks = Arc::new(InlineSystemTasks::default());
    let system_tasks_for_init = Arc::clone(&system_tasks);
    let worker = worker_thread();
    let init = DriverInit::new(worker.clone(), system_tasks_for_init);

    init.system_tasks().spawn(Box::new(|| {}));

    assert_eq!(init.thread(), &worker);
    assert_eq!(system_tasks.accepted.load(Ordering::Relaxed), 1);
}

#[test]
fn provider_creation_is_typed_and_infallible() {
    let init = driver_init();
    let driver = TestProvider.create(init);
    assert_eq!(driver.context(), TestContext(7));
}

#[test]
fn context_type_selects_provider_and_driver() {
    fn provider_for<C: DriverContext>() -> C::Provider {
        C::provider()
    }

    let provider: TestProvider = provider_for::<TestContext>();
    let driver = provider.create(driver_init());

    assert_eq!(driver.context(), TestContext(7));
}

#[test]
fn shutdown_future_begins_and_polls_shutdown() {
    let state = Rc::new(ShutdownState::default());
    let driver = LocalDriver::new(Rc::clone(&state));
    let wake_count = Arc::new(CountingWake::default());
    let waker = Waker::from(Arc::clone(&wake_count));
    let mut cx = Context::from_waker(&waker);
    let mut shutdown = pin!(driver.shutdown());

    assert_eq!(shutdown.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(state.begin_calls.get(), 1);
    assert_eq!(state.poll_calls.get(), 1);
    assert_eq!(wake_count.count.load(Ordering::Relaxed), 1);

    assert_eq!(shutdown.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(state.begin_calls.get(), 1);
    assert_eq!(state.poll_calls.get(), 2);
}

#[test]
fn different_driver_types_have_distinct_identity() {
    use std::any::TypeId;

    assert_ne!(TypeId::of::<version_one::Driver>(), TypeId::of::<version_two::Driver>());
}

#[test]
fn parker_contract_supports_latched_wakeup() {
    let parker = TestParker::default();

    parker.waker().wake_by_ref();
    parker.park(Duration::MAX);

    assert_eq!(parker.waits.load(Ordering::Relaxed), 1);
}

#[derive(Default)]
struct InlineSystemTasks {
    accepted: AtomicUsize,
}

impl SystemTaskSpawner for InlineSystemTasks {
    fn spawn(&self, task: SystemTask) {
        self.accepted.fetch_add(1, Ordering::Relaxed);
        task();
    }
}

#[derive(Debug, Default)]
struct Latch {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl Wake for Latch {
    fn wake(self: Arc<Self>) {
        let mut raised = self.raised.lock().unwrap_or_else(PoisonError::into_inner);
        *raised = true;
        self.changed.notify_one();
    }
}

#[derive(Debug, Default)]
struct TestParker {
    latch: Arc<Latch>,
    waits: AtomicUsize,
}

impl Parker for TestParker {
    fn park(&self, max_wait: Duration) {
        self.waits.fetch_add(1, Ordering::Relaxed);
        let mut raised = self.latch.raised.lock().unwrap_or_else(PoisonError::into_inner);

        if *raised {
            *raised = false;
            return;
        }

        if max_wait.is_zero() {
            return;
        }

        if max_wait == Duration::MAX {
            while !*raised {
                raised = self.latch.changed.wait(raised).unwrap_or_else(PoisonError::into_inner);
            }
        } else {
            let (next, _) = self
                .latch
                .changed
                .wait_timeout(raised, max_wait)
                .unwrap_or_else(PoisonError::into_inner);
            raised = next;
        }

        *raised = false;
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.latch))
    }
}

#[derive(Debug, Default)]
struct ShutdownState {
    begin_calls: Cell<usize>,
    poll_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    context: TestContext,
    parker: TestParker,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self {
            state,
            context: TestContext(7),
            parker: TestParker::default(),
        }
    }
}

impl Driver for LocalDriver {
    type Context = TestContext;

    fn context(&self) -> Self::Context {
        self.context.clone()
    }

    fn parker(&self) -> &dyn Parker {
        &self.parker
    }

    fn begin_shutdown(&self) {
        self.state.begin_calls.update(|calls| calls + 1);
    }

    fn poll_shutdown(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.state.poll_calls.update(|calls| calls + 1);

        if self.state.poll_calls.get() == 1 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestContext(u8);

impl ThreadAware for TestContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverContext for TestContext {
    type Provider = TestProvider;

    fn provider() -> Self::Provider {
        TestProvider
    }
}

#[derive(Clone, Debug)]
struct TestProvider;

impl ThreadAware for TestProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for TestProvider {
    type Context = TestContext;
    type Driver = LocalDriver;

    fn create(self, _init: DriverInit) -> Self::Driver {
        LocalDriver::new(Rc::default())
    }
}

#[derive(Debug, Default)]
struct CountingWake {
    count: AtomicUsize,
}

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

fn driver_init() -> DriverInit {
    DriverInit::new(worker_thread(), Arc::new(InlineSystemTasks::default()))
}

fn worker_thread() -> Thread {
    let owner = thread_aware_core::__private::v1::new_owner();
    let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
    thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node)
}

mod version_one {
    pub(super) struct Driver;
}

mod version_two {
    pub(super) struct Driver;
}
