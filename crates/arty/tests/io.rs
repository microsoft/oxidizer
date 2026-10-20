// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for Arty's runtime-integrated I/O driver hosting.

#![cfg(feature = "rt")]
#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

testing_aids::init_tracing!();

use std::collections::HashSet;
use std::error::Error as _;
use std::marker::PhantomData;
use std::ops::Deref;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex, mpsc};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};
use std::{fmt, thread};

use arty::core::{Thread, ThreadAware};
use arty::io::Io;
use arty::runtime::{Runtime, RuntimeBuilder, RuntimeOperations, WorkersPolicy};
use arty::task::{JoinError, JoinHandle, Scheduler};
use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use observed_testing::{CapturedEvent, MockProcessor, TEST_ID, test_emitter};
use static_assertions::assert_impl_all;

const WORKERS: usize = 2;

trait IoJoinHandleExt {
    type Output;

    fn join(self) -> Result<Self::Output, JoinError>;
}

impl<R> IoJoinHandleExt for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn join(self) -> Result<Self::Output, JoinError> {
        let (result_tx, result_rx) = mpsc::channel();
        thread::spawn(move || {
            _ = result_tx.send(futures::executor::block_on(self));
        });
        match result_rx.recv_timeout(testing_aids::TEST_TIMEOUT) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("I/O task join did not complete before the test deadline: {error}");
                std::process::abort();
            }
        }
    }
}
static INVARIANT_TEST_LOCK: Mutex<()> = Mutex::new(());

assert_impl_all!(Io: Clone, Send, ThreadAware);
assert_impl_all!(RuntimeBuilder: Send, Sync);

#[derive(Debug)]
struct IoError(String);

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

trait IoTestExt {
    fn try_context<C>(&self, options: &C::ContextOptions) -> Result<C, IoError>
    where
        C: IoContext,
        C::ContextOptions: Clone + ThreadAware;
}

impl IoTestExt for Io {
    fn try_context<C>(&self, options: &C::ContextOptions) -> Result<C, IoError>
    where
        C: IoContext,
        C::ContextOptions: Clone + ThreadAware,
    {
        catch_unwind(AssertUnwindSafe(|| self.context::<C>(options))).map_err(|panic| {
            let message = match panic.downcast::<String>() {
                Ok(message) => *message,
                Err(panic) => match panic.downcast::<&'static str>() {
                    Ok(message) => (*message).to_owned(),
                    Err(panic) => std::panic::resume_unwind(panic),
                },
            };
            IoError(
                message
                    .strip_prefix("failed to access Arty I/O context: ")
                    .unwrap_or(&message)
                    .to_owned(),
            )
        })
    }
}

trait Marker: Send + 'static {
    const NAME: &'static str;
}

macro_rules! markers {
    ($($name:ident),+ $(,)?) => {
        $(
            struct $name;

            impl Marker for $name {
                const NAME: &'static str = stringify!($name);
            }
        )+
    };
}

markers!(
    CacheContext,
    ConstructionFailure,
    GetFailure,
    LateConstructionFailure,
    InconsistentPrimary,
    RacePrimaryA,
    RacePrimaryB,
    PrimaryCycleFailure,
    SecondaryCycleFailure,
    OrderedPrimary,
    OrderedSecondary,
    ShutdownPrimary,
    ShutdownSecondary,
    BlockingPrimary,
    IsolatedContext,
    ForeignRelocationContext,
    AdditionalSecondaryA,
    AdditionalSecondaryB,
    ConcurrentLateFailure,
    CycleAndShutdownFailure,
    ExternalConstructionFailure,
    ExternalShutdownDuringInitialization,
    QueuedExternalShutdown,
    NestedExecutorAccess,
    TokioExternalAccess,
    SameRuntimeCrossWorker,
    ConcurrentSameContext,
    RecursiveContext,
    ConcurrentExternal,
    ConcurrentFirstFailure,
    ShutdownDuringFirstRegistration,
    ShutdownDuringLateInitialization,
    ConcurrentFailureA,
    ConcurrentFailureB,
    ReservedSecondary,
    ShutdownWhileWaiting,
    ShutdownTaskCleanup,
);

#[derive(Clone, Copy)]
enum RolePlan {
    PreferPrimary,
    Secondary,
    Inconsistent,
}

struct Scenario {
    role_plan: RolePlan,
    context_constructions: AtomicUsize,
    relocations: AtomicUsize,
    option_relocations: AtomicUsize,
    option_clones: AtomicUsize,
    option_workers: Mutex<Vec<thread::ThreadId>>,
    driver_attempts: AtomicUsize,
    initialized: AtomicUsize,
    normal_cycles: AtomicUsize,
    primary_wait_entries: AtomicUsize,
    primary_wait_timeouts: AtomicUsize,
    shutdowns: AtomicUsize,
    fail_driver_at: AtomicUsize,
    driver_failure_message: Mutex<&'static str>,
    fail_normal_cycle: AtomicBool,
    fail_shutdown: AtomicBool,
    block_primary: AtomicBool,
    block_primary_until_woken: AtomicBool,
    record_cycles: Arc<AtomicBool>,
    roles: Mutex<Vec<(thread::ThreadId, DriverRole)>>,
    owner_wakers: Mutex<Vec<Waker>>,
    cycle_log: Arc<Mutex<Vec<&'static str>>>,
    driver_wakes: AtomicUsize,
    driver_interrupts: Mutex<Vec<Waker>>,
    block_driver_at: AtomicUsize,
    block_waker_at: AtomicUsize,
    driver_gate: Mutex<DriverGate>,
    driver_gate_changed: Condvar,
    context_action: Mutex<Option<ContextAction>>,
}

struct ScenarioOptions(Arc<Scenario>);

impl Clone for ScenarioOptions {
    fn clone(&self) -> Self {
        self.option_clones.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(&self.0))
    }
}

impl Deref for ScenarioOptions {
    type Target = Scenario;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl ThreadAware for ScenarioOptions {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.option_relocations.fetch_add(1, Ordering::SeqCst);
        self.option_workers.lock().unwrap().push(thread::current().id());
        assert_eq!(thread::current().id(), destination.id());
    }
}

#[derive(Default)]
struct DriverGate {
    entered: bool,
    released: bool,
}

enum ContextAction {
    RegisterSelf { io: Io },
}

impl Scenario {
    fn options(role_plan: RolePlan) -> ScenarioOptions {
        Self::with_log_and_recording(role_plan, Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)))
    }

    fn with_log_and_recording(
        role_plan: RolePlan,
        cycle_log: Arc<Mutex<Vec<&'static str>>>,
        record_cycles: Arc<AtomicBool>,
    ) -> ScenarioOptions {
        ScenarioOptions(Arc::new(Self {
            role_plan,
            context_constructions: AtomicUsize::new(0),
            relocations: AtomicUsize::new(0),
            option_relocations: AtomicUsize::new(0),
            option_clones: AtomicUsize::new(0),
            option_workers: Mutex::new(Vec::new()),
            driver_attempts: AtomicUsize::new(0),
            initialized: AtomicUsize::new(0),
            normal_cycles: AtomicUsize::new(0),
            primary_wait_entries: AtomicUsize::new(0),
            primary_wait_timeouts: AtomicUsize::new(0),
            shutdowns: AtomicUsize::new(0),
            fail_driver_at: AtomicUsize::new(0),
            driver_failure_message: Mutex::new("driver construction failed"),
            fail_normal_cycle: AtomicBool::new(false),
            fail_shutdown: AtomicBool::new(false),
            block_primary: AtomicBool::new(false),
            block_primary_until_woken: AtomicBool::new(false),
            record_cycles,
            roles: Mutex::new(Vec::new()),
            owner_wakers: Mutex::new(Vec::new()),
            cycle_log,
            driver_wakes: AtomicUsize::new(0),
            driver_interrupts: Mutex::new(Vec::new()),
            block_driver_at: AtomicUsize::new(0),
            block_waker_at: AtomicUsize::new(0),
            driver_gate: Mutex::new(DriverGate::default()),
            driver_gate_changed: Condvar::new(),
            context_action: Mutex::new(None),
        }))
    }

    fn supports_role(&self, attempt: usize, role: DriverRole) -> bool {
        match self.role_plan {
            RolePlan::PreferPrimary => true,
            RolePlan::Inconsistent if attempt == 1 => role == DriverRole::Primary,
            RolePlan::Secondary | RolePlan::Inconsistent => role == DriverRole::Secondary,
        }
    }

    fn wake_owners(&self) {
        for waker in self.owner_wakers.lock().unwrap().iter() {
            waker.wake_by_ref();
        }
    }

    fn wake_driver_interrupts(&self) {
        for waker in self.driver_interrupts.lock().unwrap().iter() {
            waker.wake_by_ref();
        }
    }

    fn roles(&self) -> Vec<DriverRole> {
        self.roles.lock().unwrap().iter().map(|(_, role)| *role).collect()
    }

    fn driver_workers(&self) -> HashSet<thread::ThreadId> {
        self.roles.lock().unwrap().iter().map(|(worker, _)| *worker).collect()
    }

    fn option_workers(&self) -> Vec<thread::ThreadId> {
        self.option_workers.lock().unwrap().clone()
    }

    fn wait_until_driver_blocked(&self) {
        let gate = self.driver_gate.lock().unwrap();
        let (gate, timeout) = self
            .driver_gate_changed
            .wait_timeout_while(gate, testing_aids::TEST_TIMEOUT, |gate| !gate.entered)
            .unwrap();
        let entered = gate.entered;
        drop(gate);
        assert!(!timeout.timed_out(), "driver initialization did not reach its gate");
        assert!(entered);
    }

    fn release_driver(&self) {
        self.driver_gate.lock().unwrap().released = true;
        self.driver_gate_changed.notify_all();
    }

    fn block_driver_if_configured(&self, attempt: usize) {
        if self.block_driver_at.load(Ordering::SeqCst) != attempt {
            return;
        }
        self.block_at_gate();
    }

    fn block_at_gate(&self) {
        let mut gate = self.driver_gate.lock().unwrap();
        gate.entered = true;
        self.driver_gate_changed.notify_all();
        let (gate, timeout) = self
            .driver_gate_changed
            .wait_timeout_while(gate, testing_aids::TEST_TIMEOUT, |gate| !gate.released)
            .unwrap();
        let released = gate.released;
        drop(gate);
        assert!(!timeout.timed_out(), "driver initialization gate was not released");
        assert!(released);
    }

    fn block_waker_if_configured(&self, attempt: usize) {
        if self.block_waker_at.load(Ordering::SeqCst) == attempt {
            self.block_at_gate();
        }
    }

    fn register_self_from_context(&self, io: Io) {
        *self.context_action.lock().unwrap() = Some(ContextAction::RegisterSelf { io });
    }

    fn set_driver_failure_message(&self, message: &'static str) {
        *self.driver_failure_message.lock().unwrap() = message;
    }
}

struct TestContext<M: Marker> {
    scenario: Arc<Scenario>,
    thread: Option<Thread>,
    _marker: PhantomData<M>,
}

impl<M: Marker> Clone for TestContext<M> {
    fn clone(&self) -> Self {
        Self {
            scenario: Arc::clone(&self.scenario),
            thread: self.thread.clone(),
            _marker: PhantomData,
        }
    }
}

impl<M: Marker> fmt::Debug for TestContext<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(M::NAME).field("thread", &self.thread).finish_non_exhaustive()
    }
}

impl<M: Marker> ThreadAware for TestContext<M> {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        assert!(source.is_none(), "registered context clones relocate from a thread-neutral source");
        self.scenario.relocations.fetch_add(1, Ordering::SeqCst);
        self.thread = Some(destination.clone());
    }
}

impl<M: Marker> IoContext for TestContext<M> {
    type ContextOptions = ScenarioOptions;
    type Driver = TestDriver<M>;

    fn context(options: Self::ContextOptions) -> Self {
        options.context_constructions.fetch_add(1, Ordering::SeqCst);
        let action = options.context_action.lock().unwrap().take();
        match action {
            Some(ContextAction::RegisterSelf { io }) => {
                let error = io.try_context::<Self>(&options).unwrap_err();
                assert_eq!(error.to_string(), "I/O context retrieval cannot be nested during initialization");
            }
            None => {}
        }
        Self {
            scenario: options.0,
            thread: None,
            _marker: PhantomData,
        }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        assert_eq!(self.thread.as_ref(), Some(options.thread()));
        let attempt = self.scenario.driver_attempts.fetch_add(1, Ordering::SeqCst) + 1;
        self.scenario.block_driver_if_configured(attempt);
        if self.scenario.fail_driver_at.load(Ordering::SeqCst) == attempt {
            let blocked_attempt = self.scenario.block_driver_at.load(Ordering::SeqCst);
            if blocked_attempt != 0 && blocked_attempt != attempt {
                self.scenario.wait_until_driver_blocked();
            }
            return Err(DriverError::from_message(*self.scenario.driver_failure_message.lock().unwrap()));
        }

        let role = options.role();
        if !self.scenario.supports_role(attempt, role) {
            return Err(DriverError::from_message(format!("driver does not support assigned role {role:?}")));
        }
        self.scenario.roles.lock().unwrap().push((thread::current().id(), role));
        self.scenario.owner_wakers.lock().unwrap().push(options.owner_waker().clone());
        let wake = Arc::new(DriverWake::new(Arc::clone(&self.scenario)));
        self.scenario.initialized.fetch_add(1, Ordering::SeqCst);
        Ok(TestDriver {
            scenario: Arc::clone(&self.scenario),
            role,
            wake,
            attempt,
            _marker: PhantomData,
        })
    }
}

struct DriverWake {
    scenario: Arc<Scenario>,
    notified: Mutex<bool>,
    notification: Condvar,
}

impl DriverWake {
    fn new(scenario: Arc<Scenario>) -> Self {
        Self {
            scenario,
            notified: Mutex::new(false),
            notification: Condvar::new(),
        }
    }

    fn wait(&self, duration: Duration) -> bool {
        let notified = self.notified.lock().unwrap();
        let (mut notified, _) = self
            .notification
            .wait_timeout_while(notified, duration, |notified| !*notified)
            .unwrap();
        let was_notified = *notified;
        *notified = false;
        was_notified
    }
}

impl Wake for DriverWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.scenario.driver_wakes.fetch_add(1, Ordering::SeqCst);
        *self.notified.lock().unwrap() = true;
        self.notification.notify_one();
    }
}

struct TestDriver<M: Marker> {
    scenario: Arc<Scenario>,
    role: DriverRole,
    wake: Arc<DriverWake>,
    attempt: usize,
    _marker: PhantomData<M>,
}

impl<M: Marker> Driver for TestDriver<M> {
    fn waker(&self) -> Waker {
        self.scenario.block_waker_if_configured(self.attempt);
        let waker = Waker::from(Arc::clone(&self.wake));
        self.scenario.driver_interrupts.lock().unwrap().push(waker.clone());
        waker
    }

    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
        if self.role == DriverRole::Secondary {
            assert_eq!(cycle.max_wait(), Duration::ZERO);
        }
        self.scenario.normal_cycles.fetch_add(1, Ordering::SeqCst);
        if self.scenario.record_cycles.load(Ordering::SeqCst) {
            self.scenario.cycle_log.lock().unwrap().push(M::NAME);
        }
        if self.scenario.fail_normal_cycle.load(Ordering::SeqCst) {
            return Err(DriverError::from_message("normal driver cycle failed"));
        }
        if self.role == DriverRole::Primary && self.scenario.block_primary.load(Ordering::SeqCst) && !cycle.max_wait().is_zero() {
            self.scenario.primary_wait_entries.fetch_add(1, Ordering::SeqCst);
            let wait = if self.scenario.block_primary_until_woken.load(Ordering::SeqCst) {
                testing_aids::TEST_TIMEOUT
            } else {
                cycle.max_wait()
            };
            if !self.wake.wait(wait) {
                self.scenario.primary_wait_timeouts.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        self.scenario.shutdowns.fetch_add(1, Ordering::SeqCst);
        if self.scenario.fail_shutdown.load(Ordering::SeqCst) {
            Err(DriverError::from_message("driver shutdown failed"))
        } else {
            Ok(())
        }
    }
}

fn runtime(workers: usize) -> Runtime {
    Runtime::builder().workers(WorkersPolicy::exactly(workers)).build().unwrap()
}

fn runtime_with_telemetry(workers: usize) -> (Runtime, MockProcessor) {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(workers))
        .sink(sink)
        .build()
        .unwrap();
    (runtime, processor)
}

fn worker_schedulers(runtime: &Runtime, workers: usize) -> Vec<Scheduler> {
    let schedulers: Vec<_> = (0..workers)
        .map(|_| {
            runtime
                .scheduler()
                .spawn_anywhere((), |builtins, ()| async move { builtins.scheduler().clone() })
                .join()
                .unwrap()
        })
        .collect();
    assert_eq!(schedulers.len(), workers);
    schedulers
}

fn flush_workers(workers: &[Scheduler]) {
    for worker in workers {
        worker.spawn(async |_| {}).join().unwrap();
    }
}

fn event_count(processor: &MockProcessor, name: &str) -> usize {
    processor.events().iter().filter(|event| event.name() == name).count()
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + testing_aids::TEST_TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true before the test timeout");
        thread::yield_now();
    }
}

fn serialize_invariant_test() -> std::sync::MutexGuard<'static, ()> {
    INVARIANT_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[path = "io/cycles.rs"]
mod cycles;
#[path = "io/external.rs"]
mod external;
#[path = "io/failures.rs"]
mod failures;
#[path = "io/registration.rs"]
mod registration;
#[path = "io/telemetry.rs"]
mod telemetry;
