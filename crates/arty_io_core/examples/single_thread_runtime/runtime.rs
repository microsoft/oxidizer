// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use arty_io_core::{
    Cycle, Driver, DriverError, DriverOptions, DriverProvider, DriverRole, IoContext, ProviderOptions, ShutdownError, SystemTaskSpawner,
};
use thread_aware_core::{Thread, ThreadAware};

type ContextBox = Box<dyn Any + Send>;
type ContextEntry = Arc<Mutex<Option<ContextBox>>>;
type DriverStore = Vec<Box<dyn ErasedDriver>>;
type Operation = Box<dyn FnOnce(&Thread, &SystemTaskSpawner, &mut DriverStore) + Send>;
type ShutdownResult = Result<(), ShutdownError>;
type RuntimeResult = Result<(), Box<dyn Error + Send + Sync>>;

trait ErasedDriver {
    fn role(&self) -> DriverRole;
    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError>;
    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError>;
}

struct RegisteredDriver<D> {
    driver: D,
    role: DriverRole,
}

impl<D: Driver> ErasedDriver for RegisteredDriver<D> {
    fn role(&self) -> DriverRole {
        self.role
    }

    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
        self.driver.execute_cycle(cycle)
    }

    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError> {
        Driver::shutdown(self.driver)
    }
}

pub(super) struct Runtime {
    contexts: Mutex<HashMap<TypeId, ContextEntry>>,
    commands: mpsc::Sender<Operation>,
    thread: JoinHandle<RuntimeResult>,
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
                return Ok(());
            }

            run_worker(&worker, &spawner, &commands_rx)
        });

        ready_rx.recv().expect("runtime worker must report that startup completed");

        Self {
            contexts: Mutex::new(HashMap::new()),
            commands: commands_tx,
            thread,
        }
    }

    pub(super) fn get_context<C>(&self) -> Result<C, DriverError>
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
        let context = match &mut *context {
            Some(context) => context,
            empty @ None => empty.insert(Box::new(self.initialize_context::<C>()?)),
        };
        Ok(context
            .downcast_ref::<C>()
            .expect("contexts are stored under their own TypeId")
            .clone())
    }

    fn initialize_context<C>(&self) -> Result<C, DriverError>
    where
        C: IoContext,
    {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.run(move |worker, spawner, drivers| {
            let mut provider = C::provider(ProviderOptions::new());
            let options = driver_options(worker, spawner, drivers);
            let primary_permitted = options.allowed_roles().contains(&DriverRole::Primary);
            provider.relocate(None, options.thread());
            let result = provider.create(options).and_then(|creation| {
                let role = creation.role;
                let driver = creation.driver;
                let context = creation.context;
                if role == DriverRole::Primary && !primary_permitted {
                    return Err(DriverError::from_message("driver selected primary without permission"));
                }
                let mut driver = driver;
                driver.execute_cycle(&mut Cycle::new(Duration::ZERO))?;
                register_driver(drivers, driver, role);
                Ok(context)
            });
            let _ = reply_tx.send(result);
        })?;
        reply_rx
            .recv()
            .map_err(|error| DriverError::from_message(format!("runtime worker stopped before completing context registration: {error}")))?
    }

    fn run(&self, operation: impl FnOnce(&Thread, &SystemTaskSpawner, &mut DriverStore) + Send + 'static) -> Result<(), DriverError> {
        self.commands
            .send(Box::new(operation))
            .map_err(|error| DriverError::from_message(format!("runtime worker has stopped: {error}")))
    }

    pub(super) fn shutdown(self) -> RuntimeResult {
        drop(self.commands);
        self.thread.join().expect("runtime operations and driver callbacks must not panic")
    }
}

fn run_worker(worker: &Thread, spawner: &SystemTaskSpawner, commands: &mpsc::Receiver<Operation>) -> RuntimeResult {
    let mut drivers = DriverStore::new();

    let cycle_result = loop {
        let Ok(operation) = commands.recv() else {
            break Ok(());
        };
        operation(worker, spawner, &mut drivers);
        if let Err(error) = execute_driver_cycle(&mut drivers) {
            eprintln!("driver cycle failed: {error}");
            break Err(error);
        }
    };

    let shutdown_result = shutdown_drivers(drivers);
    if let Err(error) = &shutdown_result {
        eprintln!("driver shutdown failed: {error}");
    }
    cycle_result?;
    shutdown_result?;
    Ok(())
}

fn driver_options(worker: &Thread, spawner: &SystemTaskSpawner, drivers: &DriverStore) -> DriverOptions {
    let allowed_roles = if drivers.iter().any(|driver| driver.role() == DriverRole::Primary) {
        vec![DriverRole::Secondary]
    } else {
        vec![DriverRole::Primary, DriverRole::Secondary]
    };
    DriverOptions::new(worker.clone(), spawner.clone(), allowed_roles)
}

fn register_driver<D: Driver>(drivers: &mut DriverStore, driver: D, role: DriverRole) {
    drivers.push(Box::new(RegisteredDriver { driver, role }));
}

fn execute_driver_cycle(drivers: &mut DriverStore) -> Result<(), DriverError> {
    let mut cycle = Cycle::new(Duration::ZERO);
    for driver in drivers.iter_mut().filter(|driver| driver.role() == DriverRole::Secondary) {
        driver.execute_cycle(&mut cycle)?;
    }
    if let Some(primary) = drivers.iter_mut().find(|driver| driver.role() == DriverRole::Primary) {
        primary.execute_cycle(&mut cycle)?;
    }
    Ok(())
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

    use std::task::Waker;

    use arty_io_core::{
        Cycle, Driver, DriverError, DriverInstance, DriverOptions, DriverProvider, DriverRole, IoContext, ProviderOptions, ShutdownError,
    };
    use thread_aware_core::{Thread, ThreadAware};

    use super::{Runtime, register_driver};
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
        type Context = BlockingCloneContext;
        type Driver = SampleDriver;

        fn create(self, _options: DriverOptions) -> Result<DriverInstance<Self::Driver, Self::Context>, DriverError> {
            panic!("the fixture context is inserted directly into the cache");
        }
    }

    #[derive(Clone, Debug)]
    struct PrimaryOnlyContext;

    impl ThreadAware for PrimaryOnlyContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for PrimaryOnlyContext {
        type Provider = PrimaryOnlyProvider;

        fn provider(_options: ProviderOptions) -> Self::Provider {
            PrimaryOnlyProvider
        }
    }

    #[derive(Clone)]
    struct PrimaryOnlyProvider;

    impl ThreadAware for PrimaryOnlyProvider {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl DriverProvider for PrimaryOnlyProvider {
        type Context = PrimaryOnlyContext;
        type Driver = SampleDriver;

        fn create(self, _options: DriverOptions) -> Result<DriverInstance<Self::Driver, Self::Context>, DriverError> {
            Ok(DriverInstance::new(SampleDriver, PrimaryOnlyContext, DriverRole::Primary))
        }
    }

    struct ShutdownProbe {
        name: &'static str,
        shutdowns: Arc<Mutex<Vec<&'static str>>>,
        fail_cycle: bool,
        fail_shutdown: bool,
    }

    impl Driver for ShutdownProbe {
        fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
            if self.fail_cycle {
                Err(DriverError::from_message(format!("{} cycle failed", self.name)))
            } else {
                Ok(())
            }
        }

        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn shutdown(self) -> Result<(), ShutdownError> {
            self.shutdowns.lock().unwrap().push(self.name);
            if self.fail_shutdown {
                Err(ShutdownError::from_message(self.name))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn primary_permission_rejects_a_second_primary_registration() {
        let runtime = Runtime::start();
        runtime.get_context::<SampleContext>().unwrap();

        let error = runtime.get_context::<PrimaryOnlyContext>().unwrap_err();

        assert_eq!(error.to_string(), "driver selected primary without permission");
        assert!(
            runtime.contexts.lock().unwrap()[&TypeId::of::<PrimaryOnlyContext>()]
                .lock()
                .unwrap()
                .is_none()
        );
        runtime.shutdown().unwrap();
    }

    #[test]
    fn shutdown_attempts_remaining_drivers_and_returns_the_first_error() {
        let runtime = Runtime::start();
        let shutdowns = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&shutdowns);
        runtime
            .run(move |_, _, drivers| {
                for (name, role) in [("primary", DriverRole::Primary), ("secondary", DriverRole::Secondary)] {
                    register_driver(
                        drivers,
                        ShutdownProbe {
                            name,
                            shutdowns: Arc::clone(&recorded),
                            fail_cycle: false,
                            fail_shutdown: true,
                        },
                        role,
                    );
                }
            })
            .unwrap();

        let error = runtime.shutdown().unwrap_err();

        assert_eq!(
            (shutdowns.lock().unwrap().clone(), error.to_string()),
            (vec!["secondary", "primary"], "secondary".to_string())
        );
    }

    #[test]
    fn cycle_errors_shut_down_all_drivers_and_preserve_the_cause() {
        for failing_role in [DriverRole::Primary, DriverRole::Secondary] {
            for fail_shutdown in [false, true] {
                let runtime = Runtime::start();
                let shutdowns = Arc::new(Mutex::new(Vec::new()));
                let recorded = Arc::clone(&shutdowns);
                runtime
                    .run(move |_, _, drivers| {
                        for (name, role) in [
                            ("primary", DriverRole::Primary),
                            ("secondary", DriverRole::Secondary),
                            ("remaining", DriverRole::Secondary),
                        ] {
                            register_driver(
                                drivers,
                                ShutdownProbe {
                                    name,
                                    shutdowns: Arc::clone(&recorded),
                                    fail_cycle: role == failing_role && name != "remaining",
                                    fail_shutdown,
                                },
                                role,
                            );
                        }
                    })
                    .unwrap();

                let error = runtime.shutdown().unwrap_err();
                let expected_error = match failing_role {
                    DriverRole::Primary => "primary cycle failed",
                    DriverRole::Secondary => "secondary cycle failed",
                };

                assert_eq!(error.downcast_ref::<DriverError>().unwrap().to_string(), expected_error);
                assert_eq!(*shutdowns.lock().unwrap(), ["secondary", "remaining", "primary"]);
            }
        }
    }

    #[test]
    fn cycle_error_discards_queued_work_and_rejects_new_registration() {
        let runtime = Runtime::start();
        let shutdowns = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&shutdowns);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        runtime
            .run(move |_, _, drivers| {
                register_driver(
                    drivers,
                    ShutdownProbe {
                        name: "primary",
                        shutdowns: recorded,
                        fail_cycle: true,
                        fail_shutdown: false,
                    },
                    DriverRole::Primary,
                );
                ready_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap();
        ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        let (queued_tx, queued_rx) = mpsc::channel();
        runtime.run(move |_, _, _| queued_tx.send(()).unwrap()).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(
            queued_rx.recv_timeout(Duration::from_secs(10)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert_eq!(
            runtime.get_context::<SampleContext>().err().unwrap().to_string(),
            "runtime worker has stopped: sending on a closed channel"
        );
        assert!(
            runtime.contexts.lock().unwrap()[&TypeId::of::<SampleContext>()]
                .lock()
                .unwrap()
                .is_none()
        );

        let error = runtime.shutdown().unwrap_err();

        assert_eq!(error.downcast_ref::<DriverError>().unwrap().to_string(), "primary cycle failed");
        assert_eq!(*shutdowns.lock().unwrap(), ["primary"]);
    }

    #[test]
    fn blocked_clone_does_not_block_another_cached_context() {
        let runtime = Runtime::start();
        let _ = runtime.get_context::<EchoContext>().unwrap();
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
                let _ = runtime.get_context::<BlockingCloneContext>().unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(10)).unwrap();

            let (done_tx, done_rx) = mpsc::channel();
            scope.spawn(move || {
                let _ = runtime.get_context::<EchoContext>().unwrap();
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
        let _ = runtime.get_context::<SampleContext>().unwrap();
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        runtime
            .run(move |_, _, _| {
                blocked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap();
        blocked_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        thread::scope(|scope| {
            let (done_tx, done_rx) = mpsc::channel();
            let runtime = &runtime;
            scope.spawn(move || {
                let _ = runtime.get_context::<SampleContext>().unwrap();
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
                    let _ = runtime.get_context::<SampleContext>().unwrap();
                    let _ = runtime.get_context::<EchoContext>().unwrap();
                });
            }
        });

        let (count_tx, count_rx) = mpsc::channel();
        runtime
            .run(move |_, _, drivers| {
                count_tx.send(drivers.len()).unwrap();
            })
            .unwrap();
        let count = count_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        runtime.shutdown().unwrap();

        assert_eq!(count, 2);
    }
}
