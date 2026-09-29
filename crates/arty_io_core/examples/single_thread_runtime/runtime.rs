// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arty_io_core::{
    Cycle, Driver, DriverError, DriverHandle, DriverOptions, DriverProvider, DriverRole, IoContext, ProviderOptions, ShutdownError,
    SystemTaskSpawner,
};
use thread_aware_core::{Thread, ThreadAware};

use super::coordinator::Coordinator;

type ContextBox = Box<dyn Any + Send>;
type ContextEntry = Arc<Mutex<Option<ContextBox>>>;
type DriverStore = Vec<Box<dyn ErasedDriver>>;
type Operation = Box<dyn FnOnce(&Thread, &SystemTaskSpawner, &mut Coordinator, &mut DriverStore) + Send>;
type ShutdownResult = Result<(), ShutdownError>;

enum Command {
    Run(Operation),
    Stop { reply: mpsc::Sender<ShutdownResult> },
}

trait ErasedDriver {
    fn handle(&self) -> DriverHandle<'_>;
    fn on_peer_registered(&mut self, peer: DriverHandle<'_>);
    fn role(&self) -> DriverRole;
    fn execute_cycle(&mut self, cycle: &mut Cycle<'_>) -> Result<(), DriverError>;
    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError>;
}

struct RegisteredDriver<D> {
    driver: D,
    role: DriverRole,
}

impl<D: Driver> ErasedDriver for RegisteredDriver<D> {
    fn handle(&self) -> DriverHandle<'_> {
        Driver::handle(&self.driver)
    }

    fn on_peer_registered(&mut self, peer: DriverHandle<'_>) {
        Driver::on_peer_registered(&mut self.driver, peer);
    }

    fn role(&self) -> DriverRole {
        self.role
    }

    fn execute_cycle(&mut self, cycle: &mut Cycle<'_>) -> Result<(), DriverError> {
        self.driver.execute_cycle(cycle)
    }

    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError> {
        Driver::shutdown(self.driver)
    }
}

pub(super) struct Runtime {
    contexts: Mutex<HashMap<TypeId, ContextEntry>>,
    commands: mpsc::Sender<Command>,
    thread: JoinHandle<()>,
}

impl Runtime {
    pub(super) fn start() -> Self {
        let owner = thread_aware_core::__private::v1::new_owner();
        let spawner = SystemTaskSpawner::from_fn(|task| drop(thread::spawn(task)));
        let (commands_tx, commands_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
            let worker = thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node);

            if ready_tx.send(()).is_err() {
                return;
            }

            run_worker(&worker, &spawner, &commands_rx);
        });

        ready_rx.recv().expect("runtime worker must report that startup completed");

        Self {
            contexts: Mutex::new(HashMap::new()),
            commands: commands_tx,
            thread,
        }
    }

    pub(super) fn get_context<C>(&self) -> C
    where
        C: IoContext,
    {
        let entry = {
            let mut contexts = self
                .contexts
                .lock()
                .expect("a previous cache-map update panicked, leaving runtime state unusable");
            Arc::clone(contexts.entry(TypeId::of::<C>()).or_default())
        };
        let mut context = entry
            .lock()
            .expect("a previous context lookup panicked, leaving runtime state unusable");
        // Serialize registration and cloning for this context without holding the cache-map lock.
        context
            .get_or_insert_with(|| Box::new(self.initialize_context::<C>()))
            .downcast_ref::<C>()
            .expect("contexts are stored under their own TypeId")
            .clone()
    }

    fn initialize_context<C>(&self) -> C
    where
        C: IoContext,
    {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.run(move |worker, spawner, coordinator, drivers| {
            let mut provider = C::provider(ProviderOptions::new());
            let options = driver_options(worker, spawner, drivers, <C::Provider as DriverProvider>::CAN_BE_PRIMARY);
            let role = options.role();
            provider.relocate(None, options.thread());
            let (mut driver, context) = provider.create(options).expect("sample driver initialization is infallible");
            driver
                .execute_cycle(&mut Cycle::new(Instant::now(), Duration::ZERO, false, coordinator))
                .expect("sample driver initialization cycle is infallible");
            register_driver(drivers, driver, role);
            let _ = reply_tx.send(context);
        });
        reply_rx
            .recv()
            .expect("driver initialization failure must terminate context registration")
    }

    fn run(&self, operation: impl FnOnce(&Thread, &SystemTaskSpawner, &mut Coordinator, &mut DriverStore) + Send + 'static) {
        assert!(
            self.commands.send(Command::Run(Box::new(operation))).is_ok(),
            "runtime worker must remain alive while executing an operation"
        );
    }

    pub(super) fn shutdown(self) -> ShutdownResult {
        let (reply_tx, reply_rx) = mpsc::channel();
        assert!(
            self.commands.send(Command::Stop { reply: reply_tx }).is_ok(),
            "runtime worker must remain alive during shutdown"
        );
        let result = reply_rx.recv().expect("runtime worker must report driver shutdown");
        assert!(self.thread.join().is_ok(), "runtime worker must not panic");
        result
    }
}

fn run_worker(worker: &Thread, spawner: &SystemTaskSpawner, commands: &mpsc::Receiver<Command>) {
    let mut drivers = DriverStore::new();
    let mut coordinator = Coordinator;

    while let Ok(command) = commands.recv() {
        match command {
            Command::Run(operation) => {
                operation(worker, spawner, &mut coordinator, &mut drivers);
                execute_driver_cycle(&mut drivers, &mut coordinator);
            }
            Command::Stop { reply } => {
                let result = shutdown_drivers(drivers);
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn driver_options<'a>(worker: &Thread, spawner: &SystemTaskSpawner, drivers: &'a DriverStore, can_be_primary: bool) -> DriverOptions<'a> {
    let driver_handles = drivers.iter().map(|driver| driver.handle()).collect();
    let role = if can_be_primary && !drivers.iter().any(|driver| driver.role() == DriverRole::Primary) {
        DriverRole::Primary
    } else {
        DriverRole::Secondary
    };
    DriverOptions::new(worker.clone(), spawner.clone(), driver_handles, role)
}

fn register_driver<D: Driver>(drivers: &mut DriverStore, driver: D, role: DriverRole) {
    drivers.push(Box::new(RegisteredDriver { driver, role }));
    let (driver, existing_drivers) = drivers.split_last_mut().expect("the new driver was pushed immediately above");
    let driver = driver.handle();
    for existing_driver in existing_drivers {
        existing_driver.on_peer_registered(driver);
    }
}

fn execute_driver_cycle(drivers: &mut DriverStore, coordinator: &mut Coordinator) {
    let started_at = Instant::now();
    for driver in drivers.iter_mut().filter(|driver| driver.role() == DriverRole::Secondary) {
        driver
            .execute_cycle(&mut Cycle::new(started_at, Duration::ZERO, false, coordinator))
            .expect("sample driver cycle is infallible");
    }
    if let Some(primary) = drivers.iter_mut().find(|driver| driver.role() == DriverRole::Primary) {
        primary
            .execute_cycle(&mut Cycle::new(started_at, Duration::ZERO, true, coordinator))
            .expect("sample driver cycle is infallible");
    }
}

fn shutdown_drivers(drivers: DriverStore) -> ShutdownResult {
    let mut failure = None;
    let mut primary = None;
    let mut secondaries = Vec::new();
    for driver in drivers {
        match driver.role() {
            DriverRole::Primary => {
                assert!(primary.replace(driver).is_none(), "a worker must have at most one primary driver");
            }
            DriverRole::Secondary => secondaries.push(driver),
        }
    }
    for driver in secondaries.into_iter().chain(primary) {
        if let Err(error) = driver.shutdown() {
            failure.get_or_insert(error);
        }
    }

    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::sync::{Arc, Barrier, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use arty_io_core::{DriverError, DriverOptions, DriverProvider, IoContext, ProviderOptions};
    use thread_aware_core::{Thread, ThreadAware};

    use super::Runtime;
    use crate::drivers::{EchoContext, SampleContext, SampleDriver};

    struct BlockingCloneContext {
        started: mpsc::Sender<()>,
        release: Arc<Mutex<mpsc::Receiver<()>>>,
    }

    impl Clone for BlockingCloneContext {
        fn clone(&self) -> Self {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Self {
                started: self.started.clone(),
                release: Arc::clone(&self.release),
            }
        }
    }

    impl ThreadAware for BlockingCloneContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for BlockingCloneContext {
        type Provider = BlockingCloneProvider;

        fn provider(_options: ProviderOptions) -> Self::Provider {
            panic!("the fixture context is inserted directly into the cache");
        }
    }

    #[derive(Clone)]
    struct BlockingCloneProvider;

    impl ThreadAware for BlockingCloneProvider {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl DriverProvider for BlockingCloneProvider {
        const CAN_BE_PRIMARY: bool = false;

        type Context = BlockingCloneContext;
        type Driver = SampleDriver;

        fn create(self, _options: DriverOptions<'_>) -> Result<(Self::Driver, Self::Context), DriverError> {
            panic!("the fixture context is inserted directly into the cache");
        }
    }

    #[test]
    fn blocked_clone_does_not_block_another_cached_context() {
        let runtime = Runtime::start();
        let _ = runtime.get_context::<EchoContext>();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let context = BlockingCloneContext {
            started: started_tx,
            release: Arc::new(Mutex::new(release_rx)),
        };
        runtime
            .contexts
            .lock()
            .unwrap()
            .insert(TypeId::of::<BlockingCloneContext>(), Arc::new(Mutex::new(Some(Box::new(context)))));

        thread::scope(|scope| {
            let runtime = &runtime;
            scope.spawn(move || {
                let _ = runtime.get_context::<BlockingCloneContext>();
            });
            started_rx.recv_timeout(Duration::from_secs(10)).unwrap();

            let (done_tx, done_rx) = mpsc::channel();
            scope.spawn(move || {
                let _ = runtime.get_context::<EchoContext>();
                done_tx.send(()).unwrap();
            });

            let result = done_rx.recv_timeout(Duration::from_secs(10));
            release_tx.send(()).unwrap();
            result.unwrap();
        });

        runtime.shutdown().unwrap();
    }

    #[test]
    fn cached_context_does_not_wait_for_the_worker() {
        let runtime = Runtime::start();
        let _ = runtime.get_context::<SampleContext>();
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        runtime.run(move |_, _, _, _| {
            blocked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        blocked_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        thread::scope(|scope| {
            let (done_tx, done_rx) = mpsc::channel();
            let runtime = &runtime;
            scope.spawn(move || {
                let _ = runtime.get_context::<SampleContext>();
                done_tx.send(()).unwrap();
            });

            let result = done_rx.recv_timeout(Duration::from_secs(10));
            release_tx.send(()).unwrap();
            result.unwrap();
        });

        runtime.shutdown().unwrap();
    }

    #[test]
    fn concurrent_misses_register_each_context_once() {
        let runtime = Runtime::start();
        let ready = Barrier::new(4);

        thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    ready.wait();
                    let _ = runtime.get_context::<SampleContext>();
                    let _ = runtime.get_context::<EchoContext>();
                });
            }
        });

        let (count_tx, count_rx) = mpsc::channel();
        runtime.run(move |_, _, _, drivers| {
            count_tx.send(drivers.len()).unwrap();
        });
        let count = count_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        runtime.shutdown().unwrap();

        assert_eq!(count, 2);
    }
}
