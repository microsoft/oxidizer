// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::error::Error;
use std::fmt;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use arty_io_core::{Driver, DriverHandle, DriverOptions, DriverProvider, IoContext, ProviderOptions, ShutdownError, SystemTaskSpawner};
use thread_aware_core::{Thread, ThreadAware};

use super::spawner::runtime_spawner;

type ContextBox = Box<dyn Any + Send>;
type DriverStore = Vec<Box<dyn ErasedDriver>>;
type Install = Box<dyn for<'a> FnOnce(DriverOptions<'a>) -> Box<dyn ErasedDriver> + Send>;
type Initialize = Box<dyn for<'a> FnOnce(DriverOptions<'a>) -> Initialization + Send>;
type ShutdownResult = Result<(), ShutdownError>;

struct Initialization {
    context: ContextBox,
    driver: Box<dyn ErasedDriver>,
    peer_installs: Vec<(mpsc::Sender<Command>, Install)>,
}

enum Command {
    Context {
        context_type: TypeId,
        reply: mpsc::Sender<Option<ContextBox>>,
    },
    Initialize {
        context_type: TypeId,
        initialize: Initialize,
        reply: mpsc::Sender<ContextBox>,
    },
    Install {
        install: Install,
        reply: mpsc::Sender<()>,
    },
    Stop {
        reply: mpsc::Sender<ShutdownResult>,
    },
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context { .. } => f.write_str("Context"),
            Self::Initialize { .. } => f.write_str("Initialize"),
            Self::Install { .. } => f.write_str("Install"),
            Self::Stop { .. } => f.write_str("Stop"),
        }
    }
}

trait ErasedDriver {
    fn handle(&self) -> DriverHandle<'_>;
    fn on_peer_registered(&mut self, peer: DriverHandle<'_>);
    fn context_type(&self) -> TypeId;
    fn context(&self) -> ContextBox;
    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError>;
}

impl<D: Driver> ErasedDriver for D {
    fn handle(&self) -> DriverHandle<'_> {
        Driver::handle(self)
    }

    fn on_peer_registered(&mut self, peer: DriverHandle<'_>) {
        Driver::on_peer_registered(self, peer);
    }

    fn context_type(&self) -> TypeId {
        TypeId::of::<D::Context>()
    }

    fn context(&self) -> ContextBox {
        Box::new(Driver::context(self))
    }

    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError> {
        Driver::shutdown(*self)
    }
}

struct Worker {
    commands: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

pub(super) struct Runtime {
    workers: Vec<Worker>,
    stopped: bool,
}

impl Runtime {
    pub(super) const WORKER_COUNT: usize = 2;
    const NUMA_NODES: [u32; Self::WORKER_COUNT] = [0, 1];

    pub(super) fn start() -> Result<Self, RuntimeError> {
        let owner = thread_aware_core::__private::v1::new_owner();
        let spawner = runtime_spawner();
        let mut workers = Vec::with_capacity(Self::WORKER_COUNT);

        for numa_node_id in Self::NUMA_NODES {
            let (commands_tx, commands_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let worker_owner = owner.clone();
            let worker_spawner = spawner.clone();

            let thread = thread::spawn(move || {
                let numa_node = thread_aware_core::__private::v1::new_numa_node(numa_node_id);
                let worker = thread_aware_core::__private::v1::new_thread(worker_owner, thread::current().id(), numa_node);

                if ready_tx.send(()).is_err() {
                    return;
                }

                run_worker(&worker, &worker_spawner, &commands_rx);
            });

            ready_rx
                .recv()
                .map_err(|error| RuntimeError::message(format!("worker stopped during startup: {error}")))?;
            workers.push(Worker {
                commands: commands_tx,
                thread: Some(thread),
            });
        }

        Ok(Self { workers, stopped: false })
    }

    pub(super) fn get_context<C>(&self) -> C
    where
        C: IoContext,
    {
        if let Some(context) = self.try_get_context::<C>() {
            return context;
        }

        self.initialize_context::<C>()
    }

    /// Asks a worker for a context created by the driver it already owns.
    ///
    /// This small control-thread example represents calls as belonging to worker 0. A real
    /// runtime uses the driver of the worker on which `get_context` is called.
    fn try_get_context<C>(&self) -> Option<C>
    where
        C: IoContext,
    {
        let worker = self.workers.first().expect("the fixed runtime always has at least one worker");
        let (reply_tx, reply_rx) = mpsc::channel();

        worker
            .commands
            .send(Command::Context {
                context_type: TypeId::of::<C>(),
                reply: reply_tx,
            })
            .expect("a Runtime-owned worker must remain alive during context retrieval");

        let context = reply_rx.recv().expect("a worker replies before it stops serving commands")?;

        Some(downcast_context(context))
    }

    fn initialize_context<C>(&self) -> C
    where
        C: IoContext,
    {
        let worker = self.workers.first().expect("the fixed runtime always has at least one worker");
        let initialize = self.context_initializer::<C>();
        let (reply_tx, reply_rx) = mpsc::channel();

        worker
            .commands
            .send(Command::Initialize {
                context_type: TypeId::of::<C>(),
                initialize,
                reply: reply_tx,
            })
            .expect("a Runtime-owned worker must remain alive during context registration");

        let context = reply_rx
            .recv()
            .expect("driver initialization failure must terminate context registration");
        downcast_context(context)
    }

    fn context_initializer<C>(&self) -> Initialize
    where
        C: IoContext,
    {
        let peer_commands = self
            .workers
            .iter()
            .skip(1)
            .map(|worker| worker.commands.clone())
            .collect::<Vec<_>>();
        Box::new(move |options: DriverOptions<'_>| {
            let mut provider = C::provider(ProviderOptions::new());
            let peer_installs = peer_commands
                .into_iter()
                .map(|commands| {
                    let mut peer_provider = provider.clone();
                    let install: Install = Box::new(move |options: DriverOptions<'_>| {
                        peer_provider.relocate(None, options.thread());
                        Box::new(peer_provider.create(options)) as Box<dyn ErasedDriver>
                    });
                    (commands, install)
                })
                .collect();

            provider.relocate(None, options.thread());
            let driver = provider.create(options);
            let context = Box::new(Driver::context(&driver)) as ContextBox;
            Initialization {
                context,
                driver: Box::new(driver),
                peer_installs,
            }
        })
    }

    pub(super) fn shutdown(mut self) -> Result<(), RuntimeError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), RuntimeError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;

        let mut failure = None;
        let mut shutdowns = Vec::with_capacity(self.workers.len());
        for worker in &self.workers {
            let (reply_tx, reply_rx) = mpsc::channel();
            match worker.commands.send(Command::Stop { reply: reply_tx }) {
                Ok(()) => shutdowns.push(reply_rx),
                Err(error) => {
                    failure.get_or_insert_with(|| RuntimeError::message(format!("worker stopped before shutdown: {error}")));
                }
            }
        }

        for shutdown in shutdowns {
            match shutdown.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert_with(|| RuntimeError::DriverShutdown(error));
                }
                Err(error) => {
                    failure.get_or_insert_with(|| RuntimeError::message(format!("worker stopped during driver shutdown: {error}")));
                }
            }
        }

        for worker in &mut self.workers {
            match worker.thread.take() {
                Some(thread) => {
                    if thread.join().is_err() {
                        failure.get_or_insert_with(|| RuntimeError::message("worker thread panicked"));
                    }
                }
                None => {
                    failure.get_or_insert_with(|| RuntimeError::message("worker thread was already joined"));
                }
            }
        }

        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn run_worker(worker: &Thread, spawner: &SystemTaskSpawner, commands: &mpsc::Receiver<Command>) {
    let mut drivers = DriverStore::new();

    while let Ok(command) = commands.recv() {
        match command {
            Command::Context { context_type, reply } => {
                let _ = reply.send(find_context(&drivers, context_type));
            }
            Command::Initialize {
                context_type,
                initialize,
                reply,
            } => {
                // Context lookup and initialization share this queue, so this second check
                // serializes concurrent misses without a mutex.
                if let Some(context) = find_context(&drivers, context_type) {
                    let _ = reply.send(context);
                    continue;
                }

                let Initialization {
                    context,
                    driver,
                    peer_installs,
                } = {
                    let options = driver_options(worker, spawner, &drivers);
                    initialize(options)
                };
                register_driver(&mut drivers, driver);

                for (commands, install) in peer_installs {
                    let (reply_tx, reply_rx) = mpsc::channel();
                    commands
                        .send(Command::Install { install, reply: reply_tx })
                        .expect("a Runtime-owned worker must remain alive during context registration");
                    reply_rx
                        .recv()
                        .expect("driver initialization failure must terminate context registration");
                }

                let _ = reply.send(context);
            }
            Command::Install { install, reply } => {
                let driver = {
                    let options = driver_options(worker, spawner, &drivers);
                    install(options)
                };
                register_driver(&mut drivers, driver);
                let _ = reply.send(());
            }
            Command::Stop { reply } => {
                let result = shutdown_drivers(drivers);
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn find_context(drivers: &DriverStore, context_type: TypeId) -> Option<ContextBox> {
    drivers
        .iter()
        .find(|driver| driver.context_type() == context_type)
        .map(|driver| driver.context())
}

fn driver_options<'a>(worker: &Thread, spawner: &SystemTaskSpawner, drivers: &'a DriverStore) -> DriverOptions<'a> {
    let driver_handles = drivers.iter().map(|driver| driver.handle()).collect();
    DriverOptions::new(worker.clone(), spawner.clone(), driver_handles)
}

fn register_driver(drivers: &mut DriverStore, driver: Box<dyn ErasedDriver>) {
    drivers.push(driver);
    let (driver, existing_drivers) = drivers.split_last_mut().expect("the new driver was pushed immediately above");
    let driver = driver.handle();
    for existing_driver in existing_drivers {
        existing_driver.on_peer_registered(driver);
    }
}

fn downcast_context<C: IoContext>(context: ContextBox) -> C {
    *context.downcast::<C>().expect("a driver always builds its own context type")
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

#[derive(Debug)]
pub(super) enum RuntimeError {
    Message(Box<str>),
    DriverShutdown(ShutdownError),
}

impl RuntimeError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into().into_boxed_str())
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::DriverShutdown(_) => f.write_str("runtime shutdown failed"),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::DriverShutdown(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_driver::{SampleContext, created_driver_count};

    #[test]
    fn initialization_rechecks_context_after_a_miss() {
        let runtime = Runtime::start().unwrap();
        let commands = &runtime.workers.first().unwrap().commands;

        for _ in 0..2 {
            let (reply_tx, reply_rx) = mpsc::channel();
            commands
                .send(Command::Context {
                    context_type: TypeId::of::<SampleContext>(),
                    reply: reply_tx,
                })
                .unwrap();
            assert!(reply_rx.recv().unwrap().is_none());
        }

        let (first_reply_tx, first_reply_rx) = mpsc::channel();
        commands
            .send(Command::Initialize {
                context_type: TypeId::of::<SampleContext>(),
                initialize: runtime.context_initializer::<SampleContext>(),
                reply: first_reply_tx,
            })
            .unwrap();

        let (second_reply_tx, second_reply_rx) = mpsc::channel();
        commands
            .send(Command::Initialize {
                context_type: TypeId::of::<SampleContext>(),
                initialize: runtime.context_initializer::<SampleContext>(),
                reply: second_reply_tx,
            })
            .unwrap();

        let first = downcast_context::<SampleContext>(first_reply_rx.recv().unwrap());
        let second = downcast_context::<SampleContext>(second_reply_rx.recv().unwrap());
        assert_eq!(first, second);
        assert_eq!(created_driver_count(), Runtime::WORKER_COUNT);

        runtime.shutdown().unwrap();
    }
}
