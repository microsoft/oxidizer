// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Mutex, mpsc};
use std::task::Waker;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arty_io_core::{CompletionWaiter, DriverContext, DriverError, DriverProvider, IoContext, SystemTasks};
use thread_aware_core::{Thread, ThreadAware};

use super::coordinator::{Coordinator, ErasedDriver, Source};
use super::native::NativeWaiter;
use super::system_tasks::{SystemPool, report};

type ContextBox = Box<dyn Any + Send>;
type ContextCache = HashMap<TypeId, ContextBox>;
type Install = Box<dyn FnOnce(DriverContext) -> Result<Installed, DriverError> + Send>;
type Reply = mpsc::Sender<Result<(), RuntimeError>>;

struct Installed {
    context: ContextBox,
    driver: Box<dyn ErasedDriver>,
}

struct Registration<C: IoContext> {
    _provider: C::Provider,
    contexts: Vec<C>,
}

enum Command {
    #[cfg(test)]
    Pause {
        entered: mpsc::Sender<()>,
        resume: mpsc::Receiver<()>,
        cleanup: arty_io_core::SystemTask,
        accepted: mpsc::Sender<Result<(), DriverError>>,
    },
    Install {
        id: TypeId,
        install: Install,
        reply: mpsc::Sender<Result<ContextBox, DriverError>>,
    },
    Rollback {
        id: TypeId,
        deadline: Instant,
        reply: Reply,
    },
    Stop {
        deadline: Instant,
    },
}

struct Ready {
    waker: Waker,
    #[cfg(test)]
    metrics: std::sync::Arc<super::native::NativeMetrics>,
}

struct Worker {
    commands: mpsc::Sender<Command>,
    waker: Waker,
    finished: mpsc::Receiver<Result<(), RuntimeError>>,
    thread: Option<JoinHandle<()>>,
    #[cfg(test)]
    metrics: std::sync::Arc<super::native::NativeMetrics>,
}

impl Worker {
    fn send(&self, command: Command) -> Result<(), DriverError> {
        self.commands
            .send(command)
            .map_err(|error| DriverError::from_message(format!("worker control channel closed: {error}")))?;
        // Queue publication precedes the latched interruption. No submitter lock spans a wait.
        self.waker.wake_by_ref();
        Ok(())
    }

    fn finish(&mut self, deadline: Instant) -> Result<(), RuntimeError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let mut errors = Vec::new();
        match self.finished.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.extend(error.errors),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                errors.push(DriverError::from_message("worker exited without reporting its outcome"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The owner thread retains its waiter, drivers, drains, and callbacks. A timeout
                // is NOT permission to transfer or free that thread's local driver state.
                drop(thread);
                return Err(DriverError::shutdown_timeout().into());
            }
        }
        if let Err(payload) = thread.join() {
            drop(payload);
            errors.push(DriverError::from_message("an I/O owner worker panicked"));
        }
        outcome(errors)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Options {
    pub(super) quantum: NonZeroUsize,
    pub(super) shutdown_timeout: Duration,
    #[cfg(test)]
    pub(super) fail_record_worker: Option<usize>,
    #[cfg(test)]
    pub(super) missing_record_worker: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quantum: NonZeroUsize::new(8).expect("the fixed service quantum is nonzero"),
            shutdown_timeout: Duration::from_secs(5),
            #[cfg(test)]
            fail_record_worker: None,
            #[cfg(test)]
            missing_record_worker: None,
        }
    }
}

pub(super) struct Runtime {
    workers: Vec<Worker>,
    contexts: Mutex<ContextCache>,
    system: SystemPool,
    options: Options,
    stopped: bool,
}

impl Runtime {
    pub(super) const WORKER_COUNT: usize = 2;

    pub(super) fn start() -> Result<Self, RuntimeError> {
        Self::start_with(Options::default())
    }

    pub(super) fn start_with(options: Options) -> Result<Self, RuntimeError> {
        let mut runtime = Self {
            workers: Vec::with_capacity(Self::WORKER_COUNT),
            contexts: Mutex::default(),
            system: SystemPool::start()?,
            options,
            stopped: false,
        };
        let owner = thread_aware_core::__private::v1::new_owner();
        for (index, node) in [0, 1].into_iter().enumerate() {
            let (commands_tx, commands_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (finished_tx, finished) = mpsc::channel();
            let worker_owner = owner.clone();
            let tasks = runtime.system.handle();
            let execution = runtime.system.execution_lease();
            let thread = thread::Builder::new()
                .name(format!("in-memory-io-owner-{index}"))
                .spawn(move || {
                    // A controller timeout cannot revoke this owner's cleanup execution facility.
                    let _execution = execution;
                    let node = thread_aware_core::__private::v1::new_numa_node(node);
                    let worker = thread_aware_core::__private::v1::new_thread(worker_owner, thread::current().id(), node);
                    let waiter = NativeWaiter::new();
                    #[cfg(test)]
                    if options.fail_record_worker == Some(index) {
                        waiter.fail_next_record_registration();
                    }
                    #[cfg(test)]
                    if options.missing_record_worker == Some(index) {
                        waiter.disable_record_client();
                    }
                    let ready = Ready {
                        waker: waiter.waker(),
                        #[cfg(test)]
                        metrics: waiter.metrics(),
                    };
                    let result = if ready_tx.send(ready).is_ok() {
                        WorkerLoop::new(worker, tasks, waiter, commands_rx, options).run()
                    } else {
                        Err(DriverError::from_message("runtime disappeared during worker initialization").into())
                    };
                    if let Err(mpsc::SendError(Err(error))) = finished_tx.send(result) {
                        report(&format!("abandoned worker shutdown failed: {error}"));
                    }
                })
                .map_err(DriverError::from_cause)?;
            let ready = match ready_rx.recv() {
                Ok(ready) => ready,
                Err(error) => {
                    if let Err(payload) = thread.join() {
                        drop(payload);
                        report("I/O owner worker panicked during initialization");
                    }
                    return Err(DriverError::from_cause(error).into());
                }
            };
            runtime.workers.push(Worker {
                commands: commands_tx,
                waker: ready.waker,
                finished,
                thread: Some(thread),
                #[cfg(test)]
                metrics: ready.metrics,
            });
        }
        Ok(runtime)
    }

    pub(super) fn get_context<C: IoContext>(&self) -> Result<C, RuntimeError> {
        self.get_context_on::<C>(0)
    }

    pub(super) fn get_context_on<C: IoContext>(&self, index: usize) -> Result<C, RuntimeError> {
        if index >= self.workers.len() {
            return Err(DriverError::from_message("requested worker is outside this fixed runtime").into());
        }
        let mut cache = self
            .contexts
            .lock()
            .expect("a panicking registration caller poisoned the context cache");
        let id = TypeId::of::<C>();
        if let Some(registration) = cache.get(&id) {
            let registration = registration
                .downcast_ref::<Registration<C>>()
                .expect("context type ids are inserted together with their typed registration");
            return Ok(registration.contexts[index].clone());
        }

        let provider = C::provider()?;
        let mut contexts = Vec::with_capacity(self.workers.len());
        for (attempt, worker) in self.workers.iter().enumerate() {
            match install_on::<C>(worker, provider.clone(), self.options.shutdown_timeout) {
                Ok(context) => contexts.push(context),
                Err(mut failure) => {
                    // Roll back every attempted worker, including an installation whose reply
                    // timed out. FIFO commands put rollback after any such delayed creation.
                    if let Err(rollback) = self.rollback(id, attempt + 1) {
                        failure.errors.extend(rollback.errors);
                    }
                    return Err(failure);
                }
            }
        }
        let context = contexts[index].clone();
        // No caller sees a context, and no cache entry exists, until EVERY worker succeeded.
        cache.insert(
            id,
            Box::new(Registration::<C> {
                _provider: provider,
                contexts,
            }),
        );
        Ok(context)
    }

    fn rollback(&self, id: TypeId, attempted: usize) -> Result<(), RuntimeError> {
        let deadline = deadline_after(self.options.shutdown_timeout);
        let mut replies = Vec::with_capacity(attempted);
        let mut errors = Vec::new();
        for worker in self.workers.iter().take(attempted) {
            let (reply, receiver) = mpsc::channel();
            match worker.send(Command::Rollback { id, deadline, reply }) {
                Ok(()) => replies.push(receiver),
                Err(error) => errors.push(error),
            }
        }
        for reply in replies {
            match reply.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.extend(error.errors),
                Err(mpsc::RecvTimeoutError::Timeout) => errors.push(DriverError::shutdown_timeout()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    errors.push(DriverError::from_message("worker stopped before confirming registration rollback"));
                }
            }
        }
        outcome(errors)
    }

    pub(super) fn shutdown(mut self) -> Result<(), RuntimeError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), RuntimeError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let deadline = deadline_after(self.options.shutdown_timeout);
        let mut errors = Vec::new();
        match self.contexts.get_mut() {
            Ok(cache) => cache.clear(),
            Err(poisoned) => {
                errors.push(DriverError::from_message(
                    "a registration caller panicked; discarding its poisoned cache",
                ));
                // Explicitly report the poison, but still request worker cleanup during unwinding.
                poisoned.into_inner().clear();
            }
        }
        // Initiate every worker before waiting on any worker. Each owner in turn starts all
        // of its drivers' drains before servicing any of them.
        for worker in &self.workers {
            if let Err(error) = worker.send(Command::Stop { deadline }) {
                errors.push(error);
            }
        }
        for worker in &mut self.workers {
            if let Err(error) = worker.finish(deadline) {
                errors.extend(error.errors);
            }
        }
        if let Err(error) = self.system.stop(deadline) {
            errors.push(error);
        }
        outcome(errors)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            report(&format!("best-effort runtime shutdown failed: {error}"));
        }
    }
}

fn install_on<C: IoContext>(worker: &Worker, mut provider: C::Provider, timeout: Duration) -> Result<C, RuntimeError> {
    let (reply, receiver) = mpsc::channel();
    let install = Box::new(move |context: DriverContext| {
        provider.relocate(None, context.thread());
        let driver = provider.create(context)?;
        Ok(Installed {
            context: Box::new(driver.context()),
            driver: Box::new(driver),
        })
    });
    worker.send(Command::Install {
        id: TypeId::of::<C>(),
        install,
        reply,
    })?;
    let context = receiver.recv_timeout(timeout).map_err(DriverError::from_cause)??;
    Ok(*context
        .downcast::<C>()
        .expect("the install closure boxes exactly the requested context type"))
}

struct Rollback {
    deadline: Instant,
    reply: Reply,
}

struct WorkerLoop {
    worker: Thread,
    tasks: SystemTasks,
    coordinator: Coordinator<NativeWaiter>,
    commands: mpsc::Receiver<Command>,
    pending_command: Option<Command>,
    rollbacks: HashMap<TypeId, Rollback>,
    stopping: Option<Instant>,
    errors: Vec<DriverError>,
    options: Options,
}

impl WorkerLoop {
    fn new(worker: Thread, tasks: SystemTasks, waiter: NativeWaiter, commands: mpsc::Receiver<Command>, options: Options) -> Self {
        Self {
            worker,
            tasks,
            coordinator: Coordinator::new(waiter, options.quantum),
            commands,
            pending_command: None,
            rollbacks: HashMap::new(),
            stopping: None,
            errors: Vec::new(),
            options,
        }
    }

    fn run(mut self) -> Result<(), RuntimeError> {
        loop {
            self.process_commands();
            self.finish_drains();
            if self.stopping.is_some() && self.coordinator.is_empty() {
                return outcome(self.errors);
            }
            self.coordinator.service(Instant::now());
            if self.coordinator.has_failed() && self.stopping.is_none() {
                self.begin_stop(deadline_after(self.options.shutdown_timeout));
            }
            self.finish_drains();
            if self.stopping.is_some() && self.coordinator.is_empty() {
                return outcome(self.errors);
            }
            if self.pending_command.is_some() {
                continue;
            }
            if self.coordinator.wait_duration(Instant::now(), self.next_deadline()).is_zero() {
                continue;
            }
            self.coordinator.arm();
            // Arming may fail, or may publish new readiness. Neither permits a blocking wait.
            if self.coordinator.has_failed() && self.stopping.is_none() {
                continue;
            }
            if self.stopping.is_some() && self.coordinator.is_empty() {
                continue;
            }
            // System-task callbacks publish through the same per-source readiness protocol.
            // Recheck control activity AFTER arming; a later command sets the domain latch.
            match self.commands.try_recv() {
                Ok(command) => {
                    self.pending_command = Some(command);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) if self.stopping.is_none() => {
                    self.errors
                        .push(DriverError::from_message("runtime control channel disconnected without shutdown"));
                    self.begin_stop(deadline_after(self.options.shutdown_timeout));
                    continue;
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
            }
            let wait = self.coordinator.wait_duration(Instant::now(), self.next_deadline());
            if !wait.is_zero() {
                self.coordinator.collect(wait);
            }
        }
    }

    fn process_commands(&mut self) {
        for _ in 0..self.options.quantum.get() {
            let command = if let Some(command) = self.pending_command.take() {
                command
            } else {
                match self.commands.try_recv() {
                    Ok(command) => command,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if self.stopping.is_none() {
                            self.errors
                                .push(DriverError::from_message("runtime control channel disconnected without shutdown"));
                            self.begin_stop(deadline_after(self.options.shutdown_timeout));
                        }
                        break;
                    }
                }
            };
            self.command(command);
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            #[cfg(test)]
            Command::Pause {
                entered,
                resume,
                cleanup,
                accepted,
            } => {
                entered.send(()).expect("the test retains its owner-pause acknowledgement receiver");
                resume
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the test must resume its paused owner before its safety timeout");
                accepted
                    .send(self.tasks.spawn(cleanup))
                    .expect("the test retains its cleanup-admission result receiver");
            }
            Command::Install { id, install, reply } => {
                if self.stopping.is_some() {
                    send_install_reply(&reply, Err(DriverError::from_message("the owner worker is shutting down")));
                    return;
                }
                let source = Source::new(self.coordinator.waiter.waker());
                let context = DriverContext::new(self.worker.clone(), self.tasks.clone(), Waker::from(std::sync::Arc::clone(&source)));
                let context = self.coordinator.waiter.attach_clients(context);
                match context.and_then(install) {
                    Ok(installed) => {
                        self.coordinator.insert(id, source, installed.driver);
                        if reply.send(Ok(installed.context)).is_err() {
                            self.errors
                                .push(DriverError::from_message("registration caller disappeared before publication"));
                            self.begin_stop(deadline_after(self.options.shutdown_timeout));
                        }
                    }
                    Err(error) => send_install_reply(&reply, Err(error)),
                }
            }
            Command::Rollback { id, deadline, reply } => {
                if self.coordinator.begin_shutdown(id) {
                    self.rollbacks.insert(id, Rollback { deadline, reply });
                } else {
                    // Valid when this attempted worker's create returned an error.
                    send_reply(&reply, Ok(()));
                }
            }
            Command::Stop { deadline } => self.begin_stop(deadline),
        }
    }

    fn begin_stop(&mut self, deadline: Instant) {
        self.stopping = Some(self.stopping.map_or(deadline, |old| old.min(deadline)));
        self.coordinator.begin_shutdown_all();
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.rollbacks.values().map(|rollback| rollback.deadline).chain(self.stopping).min()
    }

    fn finish_drains(&mut self) {
        let now = Instant::now();
        if self.stopping.is_some_and(|deadline| now >= deadline) {
            self.coordinator.expire_all();
        } else {
            for (&id, rollback) in &self.rollbacks {
                if now >= rollback.deadline {
                    self.coordinator.expire(id);
                }
            }
        }
        self.errors.extend(self.coordinator.take_domain_errors());
        for retired in self.coordinator.take_retired() {
            if let Some(rollback) = self.rollbacks.remove(&retired.id) {
                send_reply(&rollback.reply, outcome(retired.errors));
            } else {
                self.errors.extend(retired.errors);
            }
        }
    }
}

fn send_reply(reply: &Reply, result: Result<(), RuntimeError>) {
    if let Err(mpsc::SendError(Err(error))) = reply.send(result) {
        report(&format!("unobserved registration rollback failure: {error}"));
    }
}

fn send_install_reply(reply: &mpsc::Sender<Result<ContextBox, DriverError>>, result: Result<ContextBox, DriverError>) {
    if let Err(mpsc::SendError(Err(error))) = reply.send(result) {
        report(&format!("unobserved driver creation failure: {error}"));
    }
}

fn deadline_after(timeout: Duration) -> Instant {
    let now = Instant::now();
    // This example bounds its shutdown policy even if a caller supplies an enormous duration.
    now + timeout.min(Duration::from_mins(1))
}

#[derive(Debug)]
pub(super) struct RuntimeError {
    errors: Vec<DriverError>,
}

impl From<DriverError> for RuntimeError {
    fn from(error: DriverError) -> Self {
        Self { errors: vec![error] }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index != 0 {
                f.write_str("; ")?;
            }
            error.fmt(f)?;
        }
        Ok(())
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.errors.first().map(|error| error as &(dyn Error + 'static))
    }
}

fn outcome(errors: Vec<DriverError>) -> Result<(), RuntimeError> {
    if errors.is_empty() { Ok(()) } else { Err(RuntimeError { errors }) }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
