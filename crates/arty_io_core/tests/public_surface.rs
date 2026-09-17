// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contract coverage for completion coordination and owned draining.

#![expect(clippy::unwrap_used, reason = "test failures should report the failed operation directly")]

use std::cell::Cell;
use std::error::Error;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};
use std::{fmt, io, thread};

use arty_io_core::{
    CompletionBudget, CompletionDomain, CompletionRequirements, CompletionService, CompletionWaiter, Drain, DrainStatus, Driver,
    DriverContext, DriverError, DriverProvider, IoContext, LocalDriver, ProviderContext, ServiceStatus, Shutdown, SystemTasks, WaitStatus,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

assert_not_impl_any!(DriverContext: Send, Sync);
assert_not_impl_any!(CompletionBudget: Clone, Copy);
assert_not_impl_any!(Shutdown: Send, Sync, Clone);
assert_not_impl_any!(LocalDriver<TestDriver>: Send, Sync);
assert_not_impl_any!(LocalOnlyDriver: Send, Sync);
assert_not_impl_any!(CompletionService<Rc<()>>: Send, Sync);
assert_impl_all!(ProviderContext: Clone, Send, Sync, fmt::Debug);
assert_impl_all!(CompletionDomain: Clone, Send, Sync, fmt::Debug);
assert_impl_all!(CompletionService<Arc<()>>: Clone, Send, Sync, fmt::Debug);
assert_impl_all!(DriverError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(SystemTasks: Clone, Send, Sync, fmt::Debug);
assert_impl_all!(ServiceStatus: Copy, Clone, Eq, fmt::Debug);
assert_impl_all!(TestDriver: Send, Sync);

#[test]
fn budget_is_finite_and_exhaustion_does_not_underflow() {
    let mut budget = budget(2);
    assert_eq!(budget.remaining(), 2);
    assert!(budget.try_consume());
    assert_eq!(budget.remaining(), 1);
    assert!(budget.try_consume());
    assert_eq!(budget.remaining(), 0);
    assert!(!budget.try_consume());
    assert!(!budget.try_consume());
    assert_eq!(budget.remaining(), 0);
    assert!(format!("{budget:?}").contains("CompletionBudget"));
}

#[test]
fn service_status_separates_continuation_from_deadlines() {
    let idle = ServiceStatus::idle();
    assert!(!idle.is_runnable());
    assert_eq!(idle.deadline(), None);

    let runnable = ServiceStatus::runnable();
    assert!(runnable.is_runnable());
    assert_eq!(runnable.deadline(), None);

    let deadline = Instant::now();
    let timed = ServiceStatus::at(deadline);
    assert!(!timed.is_runnable());
    assert_eq!(timed.deadline(), Some(deadline));
    assert_ne!(timed, idle);
    assert!(format!("{timed:?}").contains("ServiceStatus"));
    assert_ne!(WaitStatus::Armed, WaitStatus::WorkReady);
}

#[test]
fn domain_clones_preserve_identity_but_new_domains_are_distinct() {
    let domain = CompletionDomain::new();
    assert!(domain.is_same(&domain.clone()));
    assert!(!domain.is_same(&CompletionDomain::default()));
    assert!(format!("{domain:?}").contains("CompletionDomain"));
}

#[test]
fn typed_services_preserve_their_domain_and_local_client() {
    let domain = CompletionDomain::new();
    let value = Rc::new(Cell::new(7_usize));
    let service = domain.service(Rc::clone(&value));
    let cloned = service.clone();
    assert!(cloned.domain().is_same(&domain));
    assert!(format!("{cloned:?}").contains("CompletionService"));

    let context = driver_context(domain).with_completion_service(cloned).unwrap();
    assert!(service.domain().is_same(context.completion_domain()));
    context.completion_service::<Rc<Cell<usize>>>().unwrap().set(11);
    assert_eq!(value.get(), 11);
}

#[test]
fn foreign_domain_service_is_rejected_even_when_its_type_matches() {
    let domain = CompletionDomain::new();
    let other = CompletionDomain::new();
    let error = driver_context(domain).with_completion_service(other.service(1_u32)).unwrap_err();
    assert!(error.is_wrong_completion_domain());
    assert!(!error.is_unsupported());
}

#[test]
fn duplicate_client_type_is_rejected_instead_of_replaced() {
    let domain = CompletionDomain::new();
    let context = driver_context(domain.clone())
        .with_completion_service(domain.service(1_u32))
        .unwrap();
    let error = context.with_completion_service(domain.service(2_u32)).unwrap_err();
    assert!(error.is_duplicate_completion_service());
    assert!(!error.is_wrong_completion_domain());
}

#[test]
fn missing_client_has_a_classified_error() {
    let error = driver_context(CompletionDomain::new()).completion_service::<u32>().unwrap_err();
    assert!(error.is_unsupported());
    assert!(!error.is_duplicate_completion_service());
    assert!(error.to_string().contains("u32"));
}

#[test]
fn driver_context_exposes_worker_system_work_and_readiness() {
    let (tasks_tx, tasks_rx) = mpsc::channel();
    let tasks = SystemTasks::new(move |task| tasks_tx.send(task).unwrap());
    let wake = Arc::new(WakeCount::default());
    let domain = CompletionDomain::new();
    let worker = worker_thread();
    let context = DriverContext::new(worker.clone(), tasks, domain.clone(), Waker::from(Arc::clone(&wake)));
    let ran = Arc::new(AtomicUsize::new(0));
    let task_ran = Arc::clone(&ran);

    context.system_tasks().spawn(move || {
        // Diagnostic only; task execution and receipt provide the synchronization.
        task_ran.fetch_add(1, Ordering::Relaxed);
    });
    tasks_rx.recv().unwrap()();
    context.readiness_waker().wake_by_ref();

    assert_eq!(context.thread(), &worker);
    assert!(context.completion_domain().is_same(&domain));
    assert_eq!(wake.count(), 1);
    // Diagnostic counter read after executing the received task.
    assert_eq!(ran.load(Ordering::Relaxed), 1);
    assert!(format!("{context:?}").contains("DriverContext"));
    assert!(format!("{:?}", context.system_tasks()).contains("SystemTasks"));

    let saved = context.readiness_waker().clone();
    drop(context);
    saved.wake();
    assert_eq!(wake.count(), 2);
}

#[test]
fn provider_advertisements_are_metadata_and_clone_across_threads() {
    let context = ProviderContext::default()
        .with_completion_service::<PreferredClient>()
        .with_completion_service::<PreferredClient>();
    assert!(context.offers::<PreferredClient>());
    assert!(!context.offers::<FallbackClient>());
    let moved = context.clone();
    assert!(thread::spawn(move || moved.offers::<PreferredClient>()).join().unwrap());
    assert!(format!("{context:?}").contains("ProviderContext"));
}

#[test]
fn requirements_are_idempotent_and_validate_the_actual_context() {
    let requirements = CompletionRequirements::new()
        .require::<PreferredClient>()
        .require::<PreferredClient>();
    assert!(requirements.requires::<PreferredClient>());
    assert!(!requirements.requires::<FallbackClient>());
    assert!(format!("{requirements:?}").contains("CompletionRequirements"));

    let domain = CompletionDomain::new();
    let context = driver_context(domain.clone())
        .with_completion_service(domain.service(PreferredClient))
        .unwrap();
    let extended = requirements.clone().require::<FallbackClient>();
    assert!(extended.requires::<FallbackClient>());
    assert!(!requirements.requires::<FallbackClient>());
    assert!(extended.validate(&context).unwrap_err().is_unsupported());
    requirements.validate(&context).unwrap();
    CompletionRequirements::default().validate(&context).unwrap();

    let error = requirements.validate(&driver_context(CompletionDomain::new())).unwrap_err();
    assert!(error.is_unsupported());
}

#[test]
fn provider_selects_one_alternative_not_all_available_clients() {
    let provider = TestContext::provider(
        ProviderContext::new()
            .with_completion_service::<PreferredClient>()
            .with_completion_service::<FallbackClient>(),
    )
    .unwrap();
    let requirements = provider.completion_requirements();
    assert!(requirements.requires::<PreferredClient>());
    assert!(!requirements.requires::<FallbackClient>());

    let fallback = TestContext::provider(ProviderContext::new().with_completion_service::<FallbackClient>()).unwrap();
    assert!(fallback.completion_requirements().requires::<FallbackClient>());
    assert!(!fallback.completion_requirements().requires::<PreferredClient>());

    let error = TestContext::provider(ProviderContext::new()).unwrap_err();
    assert!(error.is_unsupported());
}

#[test]
fn creation_is_fallible_and_returns_an_owner_thread_handle() {
    let provider = TestContext::provider(ProviderContext::new().with_completion_service::<PreferredClient>()).unwrap();
    let domain = CompletionDomain::new();
    let context = driver_context(domain.clone())
        .with_completion_service(domain.service(PreferredClient))
        .unwrap();
    provider.completion_requirements().validate(&context).unwrap();
    let driver = provider.clone().create(context).unwrap();
    assert_eq!(driver.context().active_operations(), 0);
    assert!(format!("{driver:?}").contains("LocalDriver"));

    let error = provider.create(driver_context(CompletionDomain::new())).unwrap_err();
    assert!(error.is_unsupported());

    let fallback = TestContext::provider(ProviderContext::new().with_completion_service::<FallbackClient>()).unwrap();
    let domain = CompletionDomain::new();
    let context = driver_context(domain.clone())
        .with_completion_service(domain.service(FallbackClient))
        .unwrap();
    drop(fallback.create(context).unwrap());
}

#[test]
fn default_requirements_support_a_driver_without_native_clients() {
    let provider = SoftwareContext::provider(ProviderContext::new()).unwrap();
    let context = driver_context(CompletionDomain::new());
    provider.completion_requirements().validate(&context).unwrap();
    let mut driver = provider.create(context).unwrap();
    let retained = driver.context();
    assert!(!retained.0.is_closed());
    assert_eq!(driver.service(&mut budget(1)).unwrap(), ServiceStatus::idle());
    assert_eq!(driver.prepare_wait().unwrap(), WaitStatus::Armed);
    let mut shutdown = driver.shutdown();
    assert!(retained.0.is_closed());
    assert_eq!(shutdown.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
}

#[test]
fn concrete_driver_can_be_local_and_trait_object_shutdown_is_owned() {
    let driver = LocalOnlyDriver {
        inner: TestDriver::new(Waker::noop().clone()),
        _local: Rc::new(()),
    };
    let boxed: Box<dyn Driver<Context = TestContext>> = Box::new(driver);
    let driver = LocalDriver::from_box(boxed);
    let context = driver.context();
    let mut shutdown = driver.shutdown();
    assert!(context.is_closed());
    assert_eq!(shutdown.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
    assert_eq!(context.shutdown_calls(), 1);
    assert_eq!(context.drop_calls(), 1);
}

#[test]
fn service_budget_preserves_continuation_without_another_notification() {
    let mut driver = LocalDriver::new(TestDriver::new(Waker::noop().clone()));
    let context = driver.context();
    context.queue_events(3);

    assert_eq!(driver.prepare_wait().unwrap(), WaitStatus::WorkReady);
    let mut first = budget(2);
    assert_eq!(driver.service(&mut first).unwrap(), ServiceStatus::runnable());
    assert_eq!(first.remaining(), 0);
    assert_eq!(context.completed_events(), 2);

    let mut second = budget(2);
    assert_eq!(driver.service(&mut second).unwrap(), ServiceStatus::idle());
    assert_eq!(second.remaining(), 1);
    assert_eq!(context.completed_events(), 3);
    assert_eq!(driver.prepare_wait().unwrap(), WaitStatus::Armed);
}

#[test]
fn admission_closes_before_the_first_drain_turn() {
    let driver = LocalDriver::new(TestDriver::new(Waker::noop().clone()));
    let context = driver.context();
    let mut shutdown = driver.shutdown();

    assert!(context.is_closed());
    assert_eq!(context.shutdown_calls(), 1);
    assert!(context.begin_operation().unwrap_err().to_string().contains("closed"));
    assert_eq!(shutdown.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
    assert_eq!(context.drop_calls(), 1);
}

#[test]
fn active_operations_not_context_clones_keep_drain_pending() {
    let wake = Arc::new(WakeCount::default());
    let driver = LocalDriver::new(TestDriver::new(Waker::from(Arc::clone(&wake))));
    let context = driver.context();
    let retained = context.clone();
    let operation = context.begin_operation().unwrap();
    let mut shutdown = driver.shutdown();

    assert_eq!(
        shutdown.service(&mut budget(1)).unwrap(),
        DrainStatus::Pending(ServiceStatus::idle())
    );
    assert_eq!(shutdown.prepare_wait().unwrap(), WaitStatus::Armed);
    assert_eq!(context.active_operations(), 1);

    drop(operation);

    assert_eq!(wake.count(), 1);
    assert_eq!(shutdown.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
    assert_eq!(retained.active_operations(), 0);
    assert_eq!(retained.drop_calls(), 1);
}

#[test]
fn independent_drains_can_be_interleaved_with_the_same_budget_protocol() {
    let first = LocalDriver::new(TestDriver::new(Waker::noop().clone()));
    let second = LocalDriver::new(TestDriver::new(Waker::noop().clone()));
    let first_context = first.context();
    let second_context = second.context();
    first_context.queue_events(3);
    second_context.queue_events(1);
    let mut first = first.shutdown();
    let mut second = second.shutdown();

    assert_eq!(
        first.service(&mut budget(1)).unwrap(),
        DrainStatus::Pending(ServiceStatus::runnable())
    );
    assert_eq!(second.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
    assert_eq!(first_context.completed_events(), 1);
    assert_eq!(second_context.completed_events(), 1);
    assert_eq!(first.prepare_wait().unwrap(), WaitStatus::WorkReady);
    assert_eq!(first.service(&mut budget(2)).unwrap(), DrainStatus::Complete);
}

#[test]
fn dropping_a_running_driver_closes_admission_without_invalidating_operations() {
    let driver = LocalDriver::new(TestDriver::new(Waker::noop().clone()));
    let context = driver.context();
    let operation = context.begin_operation().unwrap();
    drop(driver);

    assert!(context.is_closed());
    assert_eq!(context.active_operations(), 1);
    assert_eq!(context.drop_calls(), 1);
    drop(operation);
    assert_eq!(context.active_operations(), 0);
}

#[test]
fn abandoning_a_drain_keeps_admitted_state_owned() {
    let driver = LocalDriver::new(TestDriver::new(Waker::noop().clone()));
    let context = driver.context();
    let operation = context.begin_operation().unwrap();
    let shutdown = driver.shutdown();
    assert!(format!("{shutdown:?}").contains("Shutdown"));
    drop(shutdown);

    assert!(context.is_closed());
    assert_eq!(context.shutdown_calls(), 1);
    assert_eq!(context.drop_calls(), 1);
    assert_eq!(context.active_operations(), 1);
    drop(operation);
    assert_eq!(context.active_operations(), 0);
}

#[test]
fn drain_service_failure_is_terminal_and_preserves_its_cause() {
    let drops = Rc::new(Cell::new(0));
    let mut shutdown = Shutdown::new(FailingDrain {
        drops: Rc::clone(&drops),
        fail_prepare: false,
    });
    assert_eq!(shutdown.prepare_wait().unwrap(), WaitStatus::Armed);
    let error = shutdown.service(&mut budget(1)).unwrap_err();
    assert!(error.source().unwrap().to_string().contains("native completion"));
    assert_eq!(drops.get(), 1);
    drop(shutdown);
    assert_eq!(drops.get(), 1);
}

#[test]
fn drain_preparation_failure_is_terminal() {
    let drops = Rc::new(Cell::new(0));
    let mut shutdown = Shutdown::new(FailingDrain {
        drops: Rc::clone(&drops),
        fail_prepare: true,
    });
    let error = shutdown.prepare_wait().unwrap_err();
    assert!(error.to_string().contains("notification"));
    assert_eq!(drops.get(), 1);
    drop(shutdown);
    assert_eq!(drops.get(), 1);
}

#[test]
#[should_panic(expected = "after a terminal result")]
fn completed_shutdown_cannot_be_serviced_again() {
    let mut shutdown = LocalDriver::new(TestDriver::new(Waker::noop().clone())).shutdown();
    assert_eq!(shutdown.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
    let _ = shutdown.service(&mut budget(1));
}

#[test]
#[should_panic(expected = "after a terminal result")]
fn completed_shutdown_cannot_be_armed_again() {
    let mut shutdown = LocalDriver::new(TestDriver::new(Waker::noop().clone())).shutdown();
    assert_eq!(shutdown.service(&mut budget(1)).unwrap(), DrainStatus::Complete);
    let _ = shutdown.prepare_wait();
}

#[test]
fn driver_errors_classify_recovery_without_matching_messages() {
    let error = DriverError::from_message("completion dispatch failed");
    assert!(error.to_string().contains("dispatch"));
    assert!(error.source().is_none());
    assert!(!error.is_unsupported());
    assert!(!error.is_shutdown_timeout());

    let unsupported = DriverError::unsupported("no compatible completion strategy");
    assert!(unsupported.is_unsupported());
    assert!(unsupported.source().is_none());

    let timeout = DriverError::shutdown_timeout();
    assert!(timeout.is_shutdown_timeout());
    assert!(!timeout.is_unsupported());
    assert!(format!("{timeout:?}").contains("ShutdownTimeout"));
}

#[test]
fn classified_errors_retain_a_downcastable_native_cause() {
    let error = DriverError::unsupported("the selected completion strategy is unsupported")
        .with_cause(io::Error::new(io::ErrorKind::Unsupported, "native capability unavailable"));

    assert!(error.is_unsupported());
    assert!(error.to_string().contains("selected completion strategy"));
    let cause = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(cause.kind(), io::ErrorKind::Unsupported);
}

#[test]
fn replacing_a_cause_preserves_the_error_classification() {
    let error = DriverError::shutdown_timeout()
        .with_cause(io::Error::other("first cause"))
        .with_cause(io::Error::new(io::ErrorKind::TimedOut, "native drain timed out"));

    assert!(error.is_shutdown_timeout());
    let cause = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(cause.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn nonblocking_collection_preserves_a_pending_domain_wake() {
    let mut waiter = TestWaiter::new();
    waiter.waker().wake_by_ref();
    assert_eq!(waiter.collect(Duration::ZERO, &mut budget(1)).unwrap(), ServiceStatus::idle());
    assert!(*waiter.latch.raised.lock().unwrap());

    assert_eq!(waiter.collect(Duration::MAX, &mut budget(1)).unwrap(), ServiceStatus::idle());
    assert!(!*waiter.latch.raised.lock().unwrap());
}

#[test]
fn exhausted_collection_budget_never_enters_a_wait() {
    let mut waiter = TestWaiter::new();
    let mut allowance = budget(1);
    assert!(allowance.try_consume());
    assert_eq!(waiter.collect(Duration::MAX, &mut allowance).unwrap(), ServiceStatus::runnable());
}

#[test]
fn waiter_is_boxable_and_saved_waker_is_safe_after_destruction() {
    let waiter: Box<dyn CompletionWaiter> = Box::new(TestWaiter::new());
    let domain = waiter.domain().clone();
    assert!(waiter.domain().is_same(&domain));
    let waker = waiter.waker();
    drop(waiter);
    waker.wake();
}

#[derive(Default)]
struct WakeCount {
    count: AtomicUsize,
}

impl WakeCount {
    fn count(&self) -> usize {
        // Counts observations only; it does not publish other state.
        self.count.load(Ordering::Relaxed)
    }
}

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // The counter is diagnostic and does not synchronize driver state.
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
struct PreferredClient;

#[derive(Clone, Debug)]
struct FallbackClient;

#[derive(Clone, Copy, Debug)]
enum Strategy {
    Preferred,
    Fallback,
}

#[derive(Clone, Debug)]
struct TestProvider {
    strategy: Strategy,
}

impl ThreadAware for TestProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for TestProvider {
    type Context = TestContext;
    type Driver = TestDriver;

    fn completion_requirements(&self) -> CompletionRequirements {
        match self.strategy {
            Strategy::Preferred => CompletionRequirements::new().require::<PreferredClient>(),
            Strategy::Fallback => CompletionRequirements::new().require::<FallbackClient>(),
        }
    }

    fn create(self, context: DriverContext) -> Result<LocalDriver<Self::Driver>, DriverError> {
        self.completion_requirements().validate(&context)?;
        Ok(LocalDriver::new(TestDriver::new(context.readiness_waker().clone())))
    }
}

#[derive(Clone, Debug)]
struct TestContext {
    state: Arc<Mutex<Lifecycle>>,
    readiness: Waker,
}

impl ThreadAware for TestContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for TestContext {
    type Provider = TestProvider;

    fn provider(context: ProviderContext) -> Result<Self::Provider, DriverError> {
        let strategy = if context.offers::<PreferredClient>() {
            Strategy::Preferred
        } else if context.offers::<FallbackClient>() {
            Strategy::Fallback
        } else {
            return Err(DriverError::unsupported("no compatible test completion strategy"));
        };
        Ok(TestProvider { strategy })
    }
}

impl TestContext {
    fn begin_operation(&self) -> Result<OperationLease, DriverError> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(DriverError::from_message("driver admission is closed"));
        }
        state.active += 1;
        Ok(OperationLease { context: self.clone() })
    }

    fn queue_events(&self, count: usize) {
        self.state.lock().unwrap().queued_events += count;
        self.readiness.wake_by_ref();
    }

    fn active_operations(&self) -> usize {
        self.state.lock().unwrap().active
    }

    fn completed_events(&self) -> usize {
        self.state.lock().unwrap().completed_events
    }

    fn shutdown_calls(&self) -> usize {
        self.state.lock().unwrap().shutdown_calls
    }

    fn drop_calls(&self) -> usize {
        self.state.lock().unwrap().drop_calls
    }

    fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
}

#[derive(Debug)]
struct OperationLease {
    context: TestContext,
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        self.context.state.lock().unwrap().active -= 1;
        self.context.readiness.wake_by_ref();
    }
}

#[derive(Debug, Default)]
struct Lifecycle {
    active: usize,
    closed: bool,
    queued_events: usize,
    completed_events: usize,
    shutdown_calls: usize,
    drop_calls: usize,
}

#[derive(Debug)]
struct TestDriver {
    context: TestContext,
}

impl TestDriver {
    fn new(readiness: Waker) -> Self {
        Self {
            context: TestContext {
                state: Arc::default(),
                readiness,
            },
        }
    }
}

impl Drop for TestDriver {
    fn drop(&mut self) {
        let mut state = self.context.state.lock().unwrap();
        state.closed = true;
        state.drop_calls += 1;
    }
}

impl Driver for TestDriver {
    type Context = TestContext;

    fn context(&self) -> Self::Context {
        self.context.clone()
    }

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        let mut state = self.context.state.lock().unwrap();
        while state.queued_events != 0 && budget.try_consume() {
            state.queued_events -= 1;
            state.completed_events += 1;
        }
        Ok(if state.queued_events == 0 {
            ServiceStatus::idle()
        } else {
            ServiceStatus::runnable()
        })
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        Ok(if self.context.state.lock().unwrap().queued_events == 0 {
            WaitStatus::Armed
        } else {
            WaitStatus::WorkReady
        })
    }

    fn shutdown(self: Box<Self>) -> Shutdown {
        {
            let mut state = self.context.state.lock().unwrap();
            state.closed = true;
            state.shutdown_calls += 1;
        }
        Shutdown::new(TestDrain { driver: self })
    }
}

struct LocalOnlyDriver {
    inner: TestDriver,
    _local: Rc<()>,
}

impl Driver for LocalOnlyDriver {
    type Context = TestContext;

    fn context(&self) -> Self::Context {
        self.inner.context()
    }

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        self.inner.service(budget)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.inner.prepare_wait()
    }

    fn shutdown(self: Box<Self>) -> Shutdown {
        Box::new(self.inner).shutdown()
    }
}

#[derive(Clone, Debug)]
struct SoftwareContext(TestContext);

impl ThreadAware for SoftwareContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for SoftwareContext {
    type Provider = SoftwareProvider;

    fn provider(_context: ProviderContext) -> Result<Self::Provider, DriverError> {
        Ok(SoftwareProvider)
    }
}

#[derive(Clone, Debug)]
struct SoftwareProvider;

impl ThreadAware for SoftwareProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for SoftwareProvider {
    type Context = SoftwareContext;
    type Driver = SoftwareDriver;

    fn create(self, context: DriverContext) -> Result<LocalDriver<Self::Driver>, DriverError> {
        Ok(LocalDriver::new(SoftwareDriver(TestDriver::new(context.readiness_waker().clone()))))
    }
}

struct SoftwareDriver(TestDriver);

impl Driver for SoftwareDriver {
    type Context = SoftwareContext;

    fn context(&self) -> Self::Context {
        SoftwareContext(self.0.context())
    }

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        self.0.service(budget)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.0.prepare_wait()
    }

    fn shutdown(self: Box<Self>) -> Shutdown {
        Box::new(self.0).shutdown()
    }
}

struct TestDrain {
    driver: Box<TestDriver>,
}

impl Drain for TestDrain {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        let status = self.driver.service(budget)?;
        let state = self.driver.context.state.lock().unwrap();
        Ok(if state.active == 0 && state.queued_events == 0 {
            DrainStatus::Complete
        } else {
            DrainStatus::Pending(status)
        })
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.driver.prepare_wait()
    }
}

struct FailingDrain {
    drops: Rc<Cell<usize>>,
    fail_prepare: bool,
}

impl Drain for FailingDrain {
    fn service(&mut self, _budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        Err(DriverError::from_cause(io::Error::other("native completion failed")))
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        if self.fail_prepare {
            Err(DriverError::from_message("native notification failed"))
        } else {
            Ok(WaitStatus::Armed)
        }
    }
}

impl Drop for FailingDrain {
    fn drop(&mut self) {
        self.drops.update(|count| count + 1);
    }
}

#[derive(Default)]
struct Latch {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl Wake for Latch {
    fn wake(self: Arc<Self>) {
        let mut raised = self.raised.lock().unwrap();
        *raised = true;
        self.changed.notify_one();
    }
}

struct TestWaiter {
    domain: CompletionDomain,
    latch: Arc<Latch>,
}

impl TestWaiter {
    fn new() -> Self {
        Self {
            domain: CompletionDomain::new(),
            latch: Arc::default(),
        }
    }
}

impl CompletionWaiter for TestWaiter {
    fn domain(&self) -> &CompletionDomain {
        &self.domain
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.latch))
    }

    fn collect(&mut self, max_wait: Duration, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        if budget.remaining() == 0 {
            return Ok(ServiceStatus::runnable());
        }
        if max_wait.is_zero() {
            return Ok(ServiceStatus::idle());
        }
        let mut raised = self.latch.raised.lock().unwrap();
        if max_wait == Duration::MAX {
            raised = self.latch.changed.wait_while(raised, |raised| !*raised).unwrap();
        } else if !*raised {
            (raised, _) = self.latch.changed.wait_timeout_while(raised, max_wait, |raised| !*raised).unwrap();
        }
        if *raised {
            assert!(budget.try_consume());
            *raised = false;
        }
        Ok(ServiceStatus::idle())
    }
}

fn budget(units: usize) -> CompletionBudget {
    CompletionBudget::new(NonZeroUsize::new(units).unwrap())
}

fn driver_context(domain: CompletionDomain) -> DriverContext {
    DriverContext::new(
        worker_thread(),
        SystemTasks::new(|task| drop(thread::spawn(task))),
        domain,
        Waker::noop().clone(),
    )
}

fn worker_thread() -> Thread {
    let owner = thread_aware_core::__private::v1::new_owner();
    let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
    thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node)
}
