// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};

use arty_io_core::{Driver, DriverContext, DriverProvider, IoContext, ProviderContext, ShutdownError, SystemTasks};
use thread_aware_core::{Thread, ThreadAware};

use super::system_tasks::runtime_system_tasks;

type ContextBox = Box<dyn Any + Send>;
type ContextCache = HashMap<TypeId, ContextBox>;
type DriverStore = Vec<Box<dyn ErasedDriver>>;
type Install = Box<dyn FnOnce(DriverContext, &mut DriverStore) -> ContextBox + Send>;
type ShutdownResult = Result<(), ShutdownError>;

enum Command {
    Install { install: Install, reply: mpsc::Sender<ContextBox> },
    Stop { reply: mpsc::Sender<ShutdownResult> },
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Install { .. } => f.write_str("Install"),
            Self::Stop { .. } => f.write_str("Stop"),
        }
    }
}

trait ErasedDriver {
    fn shutdown(self: Box<Self>) -> Result<(), ShutdownError>;
}

impl<D: Driver> ErasedDriver for D {
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
    contexts: Mutex<ContextCache>,
    stopped: bool,
}

impl Runtime {
    pub(super) const WORKER_COUNT: usize = 2;
    const NUMA_NODES: [u32; Self::WORKER_COUNT] = [0, 1];

    pub(super) fn start() -> Result<Self, RuntimeError> {
        let owner = thread_aware_core::__private::v1::new_owner();
        let system_tasks = runtime_system_tasks();
        let mut workers = Vec::with_capacity(Self::WORKER_COUNT);

        for numa_node_id in Self::NUMA_NODES {
            let (commands_tx, commands_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let worker_owner = owner.clone();
            let worker_system_tasks = system_tasks.clone();

            let thread = thread::spawn(move || {
                let numa_node = thread_aware_core::__private::v1::new_numa_node(numa_node_id);
                let worker = thread_aware_core::__private::v1::new_thread(worker_owner, thread::current().id(), numa_node);

                if ready_tx.send(()).is_err() {
                    return;
                }

                run_worker(&worker, &worker_system_tasks, &commands_rx);
            });

            ready_rx
                .recv()
                .map_err(|error| RuntimeError::message(format!("worker stopped during startup: {error}")))?;
            workers.push(Worker {
                commands: commands_tx,
                thread: Some(thread),
            });
        }

        Ok(Self {
            workers,
            contexts: Mutex::default(),
            stopped: false,
        })
    }

    pub(super) fn get_context<C>(&self) -> C
    where
        C: IoContext,
    {
        // Keep the guard for the entire registration. If any worker fails to initialize, the
        // resulting panic poisons this mutex and permanently prevents another registration attempt.
        let mut cache = self
            .contexts
            .lock()
            .expect("a failed driver registration makes the runtime unusable");

        if let Some(context) = cache.get(&TypeId::of::<C>()).and_then(|context| context.downcast_ref::<C>()) {
            return context.clone();
        }

        let provider = C::provider(ProviderContext::new());
        let mut caller_context = None;

        for worker in &self.workers {
            let mut worker_provider = provider.clone();
            let (reply_tx, reply_rx) = mpsc::channel();
            let install = Box::new(move |context: DriverContext, drivers: &mut DriverStore| {
                worker_provider.relocate(None, context.thread());
                let driver = worker_provider.create(context);
                let context = driver.context();
                drivers.push(Box::new(driver));
                Box::new(context) as ContextBox
            });

            worker
                .commands
                .send(Command::Install { install, reply: reply_tx })
                .expect("a Runtime-owned worker must remain alive during context registration");

            let context = reply_rx
                .recv()
                .expect("driver initialization failure must terminate context registration");
            let context = *context
                .downcast::<C>()
                .expect("the install closure always boxes the requested context type");
            caller_context.get_or_insert(context);
        }

        // This small control-thread example represents calls as belonging to worker 0. A real
        // runtime selects the context of the worker on which get_context is called.
        let context = caller_context.expect("the fixed runtime always has at least one worker");
        cache.insert(TypeId::of::<C>(), Box::new(context.clone()));
        context
    }

    pub(super) fn shutdown(mut self) -> Result<(), RuntimeError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), RuntimeError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;

        self.contexts.get_mut().unwrap_or_else(PoisonError::into_inner).clear();

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

fn run_worker(worker: &Thread, system_tasks: &SystemTasks, commands: &mpsc::Receiver<Command>) {
    let mut drivers = DriverStore::new();

    while let Ok(command) = commands.recv() {
        match command {
            Command::Install { install, reply } => {
                let context = DriverContext::new(worker.clone(), system_tasks.clone());
                let context = install(context, &mut drivers);
                let _ = reply.send(context);
            }
            Command::Stop { reply } => {
                let result = shutdown_drivers(drivers);
                let _ = reply.send(result);
                return;
            }
        }
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
