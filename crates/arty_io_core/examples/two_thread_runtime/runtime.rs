// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};

use arty_io_core::{Driver, DriverContext, DriverInit, DriverProvider, SystemTaskSpawner};
use thread_aware_core::{Thread, ThreadAware};

use super::system_tasks::RuntimeSystemTasks;

type ContextBox = Box<dyn Any + Send>;
type ContextCache = HashMap<TypeId, ContextBox>;
type DriverStore = Vec<Box<dyn Any>>;
type Install = Box<dyn FnOnce(DriverInit, &mut DriverStore) -> ContextBox + Send>;

enum Command {
    Install { install: Install, reply: mpsc::Sender<ContextBox> },
    Stop,
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Install { .. } => f.write_str("Install"),
            Self::Stop => f.write_str("Stop"),
        }
    }
}

struct Worker {
    commands: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

pub(super) struct Runtime {
    workers: Vec<Worker>,
    contexts: Mutex<ContextCache>,
}

impl Runtime {
    pub(super) const WORKER_COUNT: usize = 2;
    const NUMA_NODES: [u32; Self::WORKER_COUNT] = [0, 1];

    pub(super) fn start() -> Result<Self, RuntimeError> {
        let owner = thread_aware_core::__private::v1::new_owner();
        let system_tasks: Arc<dyn SystemTaskSpawner> = Arc::new(RuntimeSystemTasks);
        let mut workers = Vec::with_capacity(Self::WORKER_COUNT);

        for numa_node_id in Self::NUMA_NODES {
            let (commands_tx, commands_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let worker_owner = owner.clone();
            let worker_system_tasks = Arc::clone(&system_tasks);

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
                .map_err(|error| RuntimeError(format!("worker stopped during startup: {error}")))?;
            workers.push(Worker {
                commands: commands_tx,
                thread: Some(thread),
            });
        }

        Ok(Self {
            workers,
            contexts: Mutex::default(),
        })
    }

    pub(super) fn get_context<C>(&self) -> Vec<C>
    where
        C: DriverContext,
    {
        let mut cache = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(contexts) = cache.get(&TypeId::of::<C>()).and_then(|contexts| contexts.downcast_ref::<Vec<C>>()) {
            return contexts.clone();
        }

        let provider = C::provider();
        let mut contexts = Vec::with_capacity(self.workers.len());

        for worker in &self.workers {
            let mut worker_provider = provider.clone();
            let (reply_tx, reply_rx) = mpsc::channel();
            let install = Box::new(move |init: DriverInit, drivers: &mut DriverStore| {
                worker_provider.relocate(None, init.thread());
                let driver = worker_provider.create(init);
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
            contexts.push(
                *context
                    .downcast::<C>()
                    .expect("the install closure always boxes the requested context type"),
            );
        }

        cache.insert(TypeId::of::<C>(), Box::new(contexts.clone()));
        contexts
    }

    pub(super) fn shutdown(mut self) -> Result<(), RuntimeError> {
        for worker in &self.workers {
            worker
                .commands
                .send(Command::Stop)
                .map_err(|error| RuntimeError(format!("worker stopped before shutdown: {error}")))?;
        }

        for worker in &mut self.workers {
            worker
                .thread
                .take()
                .ok_or_else(|| RuntimeError("worker thread was already joined".into()))?
                .join()
                .map_err(|_panic| RuntimeError("worker thread panicked".into()))?;
        }

        Ok(())
    }
}

fn run_worker(worker: &Thread, system_tasks: &Arc<dyn SystemTaskSpawner>, commands: &mpsc::Receiver<Command>) {
    let mut drivers = DriverStore::new();

    while let Ok(command) = commands.recv() {
        match command {
            Command::Install { install, reply } => {
                let init = DriverInit::new(worker.clone(), Arc::clone(system_tasks));
                let context = install(init, &mut drivers);
                let _ = reply.send(context);
            }
            Command::Stop => return,
        }
    }
}

#[derive(Debug)]
pub(super) struct RuntimeError(String);

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for RuntimeError {}
