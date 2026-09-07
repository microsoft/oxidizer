// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task::{Context, Poll};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arty_io_core::{Driver, DriverContext, DriverInit, DriverProvider, Parker, SystemTaskSpawner};
use thread_aware_core::{Thread, ThreadAware};

use super::system_tasks::RuntimeSystemTasks;

type ContextBox = Box<dyn Any + Send>;
type ContextCache = HashMap<TypeId, ContextBox>;
type DriverStore = Vec<Box<dyn ErasedDriver>>;
type Install = Box<dyn FnOnce(DriverInit, &mut DriverStore) -> ContextBox + Send>;
type ShutdownResult = Result<(), String>;

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
    fn begin_shutdown(&self);
    fn poll_shutdown(&self, cx: &mut Context<'_>) -> Poll<()>;
    fn parker(&self) -> &dyn Parker;
}

impl<D: Driver> ErasedDriver for D {
    fn begin_shutdown(&self) {
        Driver::begin_shutdown(self);
    }

    fn poll_shutdown(&self, cx: &mut Context<'_>) -> Poll<()> {
        Driver::poll_shutdown(self, cx)
    }

    fn parker(&self) -> &dyn Parker {
        Driver::parker(self)
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

    pub(super) fn get_context<C>(&self) -> C
    where
        C: DriverContext,
    {
        let mut cache = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(context) = cache.get(&TypeId::of::<C>()).and_then(|context| context.downcast_ref::<C>()) {
            return context.clone();
        }

        let provider = C::provider();
        let mut caller_context = None;

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
        self.contexts.get_mut().unwrap_or_else(PoisonError::into_inner).clear();

        let mut shutdowns = Vec::with_capacity(self.workers.len());
        for worker in &self.workers {
            let (reply_tx, reply_rx) = mpsc::channel();
            worker
                .commands
                .send(Command::Stop { reply: reply_tx })
                .map_err(|error| RuntimeError(format!("worker stopped before shutdown: {error}")))?;
            shutdowns.push(reply_rx);
        }

        for shutdown in shutdowns {
            shutdown
                .recv()
                .map_err(|error| RuntimeError(format!("worker stopped during driver shutdown: {error}")))?
                .map_err(RuntimeError)?;
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
            Command::Stop { reply } => {
                let result = shutdown_drivers(&drivers);
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn shutdown_drivers(drivers: &DriverStore) -> ShutdownResult {
    const TIMEOUT: Duration = Duration::from_secs(1);
    const MAX_PARK: Duration = Duration::from_millis(10);

    for driver in drivers {
        driver.begin_shutdown();
    }

    let deadline = Instant::now() + TIMEOUT;
    loop {
        let mut all_ready = true;

        for driver in drivers {
            let waker = driver.parker().waker();
            let mut cx = Context::from_waker(&waker);

            if driver.poll_shutdown(&mut cx).is_pending() {
                all_ready = false;
                let remaining = deadline.saturating_duration_since(Instant::now());

                if remaining.is_zero() {
                    return Err("driver shutdown timed out".into());
                }

                driver.parker().park(remaining.min(MAX_PARK));
            }
        }

        if all_ready {
            return Ok(());
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
