// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use arty_io_core::{Driver, DriverHandle, DriverOptions, DriverProvider, IoContext, ProviderOptions, ShutdownError, SystemTaskSpawner};
use thread_aware_core::{Thread, ThreadAware};

type ContextBox = Box<dyn Any + Send>;
type DriverStore = Vec<Box<dyn ErasedDriver>>;
type Operation = Box<dyn FnOnce(&Thread, &SystemTaskSpawner, &mut DriverStore) + Send>;
type ShutdownResult = Result<(), ShutdownError>;

enum Command {
    Run(Operation),
    Stop { reply: mpsc::Sender<ShutdownResult> },
}

trait ErasedDriver {
    fn handle(&self) -> DriverHandle<'_>;
    fn on_peer_registered(&mut self, peer: DriverHandle<'_>);
    fn context_type(&self) -> TypeId;
    fn context(&self) -> ContextBox;
    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError>;
}

struct RegisteredDriver<D, C> {
    driver: D,
    context: C,
}

impl<D: Driver, C: IoContext> ErasedDriver for RegisteredDriver<D, C> {
    fn handle(&self) -> DriverHandle<'_> {
        Driver::handle(&self.driver)
    }

    fn on_peer_registered(&mut self, peer: DriverHandle<'_>) {
        Driver::on_peer_registered(&mut self.driver, peer);
    }

    fn context_type(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn context(&self) -> ContextBox {
        Box::new(self.context.clone())
    }

    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError> {
        Driver::shutdown(self.driver)
    }
}

pub(super) struct Runtime {
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
            commands: commands_tx,
            thread,
        }
    }

    pub(super) fn get_context<C>(&self) -> C
    where
        C: IoContext,
    {
        self.try_get_context::<C>().unwrap_or_else(|| self.initialize_context::<C>())
    }

    fn try_get_context<C>(&self) -> Option<C>
    where
        C: IoContext,
    {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.run(move |_, _, drivers| {
            let _ = reply_tx.send(find_context::<C>(drivers));
        });
        reply_rx.recv().expect("a worker replies before it stops serving commands")
    }

    fn initialize_context<C>(&self) -> C
    where
        C: IoContext,
    {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.run(move |worker, spawner, drivers| {
            // The lookup and initialization operations share this queue, so rechecking here
            // serializes concurrent misses without a mutex.
            if let Some(context) = find_context::<C>(drivers) {
                let _ = reply_tx.send(context);
                return;
            }

            let mut provider = C::provider(ProviderOptions::new());
            let options = driver_options(worker, spawner, drivers);
            provider.relocate(None, options.thread());
            let (driver, context) = provider.create(options);
            let reply_context = context.clone();
            register_driver(drivers, driver, context);
            let _ = reply_tx.send(reply_context);
        });
        reply_rx
            .recv()
            .expect("driver initialization failure must terminate context registration")
    }

    fn run(&self, operation: impl FnOnce(&Thread, &SystemTaskSpawner, &mut DriverStore) + Send + 'static) {
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

    while let Ok(command) = commands.recv() {
        match command {
            Command::Run(operation) => operation(worker, spawner, &mut drivers),
            Command::Stop { reply } => {
                let result = shutdown_drivers(drivers);
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn find_context<C: IoContext>(drivers: &DriverStore) -> Option<C> {
    drivers
        .iter()
        .find(|driver| driver.context_type() == TypeId::of::<C>())
        .map(|driver| {
            *driver
                .context()
                .downcast::<C>()
                .expect("context type matched by TypeId immediately above")
        })
}

fn driver_options<'a>(worker: &Thread, spawner: &SystemTaskSpawner, drivers: &'a DriverStore) -> DriverOptions<'a> {
    let driver_handles = drivers.iter().map(|driver| driver.handle()).collect();
    DriverOptions::new(worker.clone(), spawner.clone(), driver_handles)
}

fn register_driver<D: Driver, C: IoContext>(drivers: &mut DriverStore, driver: D, context: C) {
    drivers.push(Box::new(RegisteredDriver { driver, context }));
    let (driver, existing_drivers) = drivers.split_last_mut().expect("the new driver was pushed immediately above");
    let driver = driver.handle();
    for existing_driver in existing_drivers {
        existing_driver.on_peer_registered(driver);
    }
}

fn shutdown_drivers(drivers: DriverStore) -> ShutdownResult {
    let mut failure = None;

    for driver in drivers {
        if let Err(error) = driver.shutdown() {
            failure.get_or_insert(error);
        }
    }

    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
