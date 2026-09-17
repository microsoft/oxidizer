// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Readiness driver with a private completion queue and private text requests.

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::task::Waker;
use std::thread::{self, ThreadId};

use arty_io_core::{
    CompletionBudget, Drain, DrainStatus, Driver, DriverContext, DriverError, DriverProvider, IoContext, ServiceStatus, SystemTasks,
    WaitStatus,
};
use thread_aware_core::{Thread, ThreadAware};

use super::native::{ReadinessClient, ReadinessRegistration};
use super::system_tasks::Cleanup;

const MAX_TEXT_BYTES: usize = 1024;

#[derive(Debug)]
struct TextRequest {
    input: String,
    result: Option<String>,
    interested: bool,
}

#[derive(Debug)]
struct PrivateQueue {
    open: bool,
    alive: bool,
    next_token: u64,
    active: usize,
    completed_on: Option<ThreadId>,
    requests: HashMap<u64, TextRequest>,
    completions: VecDeque<u64>,
}

#[derive(Debug)]
struct EchoState {
    owner: ThreadId,
    queue: Mutex<PrivateQueue>,
    completed: Condvar,
    registration: ReadinessRegistration,
}

#[derive(Clone, Debug)]
pub(super) struct EchoContext {
    state: Arc<EchoState>,
}

impl EchoContext {
    pub(super) fn driver_thread(&self) -> ThreadId {
        self.state.owner
    }

    pub(super) fn submit(&self, input: &str) -> Result<EchoOperation, EchoIoError> {
        let mut queue = self
            .state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock");
        if !queue.open {
            return Err(EchoIoError::Closed);
        }
        if input.len() > MAX_TEXT_BYTES {
            return Err(EchoIoError::TooLong);
        }
        let token = queue.next_token;
        queue.next_token = token
            .checked_add(1)
            .ok_or_else(|| EchoIoError::Native(DriverError::from_message("echo request identifiers are exhausted")))?;
        queue.requests.insert(
            token,
            TextRequest {
                input: input.to_owned(),
                result: None,
                interested: true,
            },
        );
        queue.completions.push_back(token);
        // Publish the private entry before posting the readiness edge. The admission lock also
        // ensures shutdown includes this operation, or rejects it before any entry is published.
        if let Err(error) = self.state.registration.post() {
            queue.completions.pop_back();
            queue.requests.remove(&token);
            return Err(EchoIoError::Native(error));
        }
        queue.active += 1;
        Ok(EchoOperation {
            state: Arc::clone(&self.state),
            token,
        })
    }

    #[cfg(test)]
    pub(super) fn completed_on(&self) -> Option<ThreadId> {
        self.state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock")
            .completed_on
    }
}

impl PartialEq for EchoContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for EchoContext {}

impl ThreadAware for EchoContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for EchoContext {
    type Provider = EchoProvider;

    fn provider() -> Result<Self::Provider, DriverError> {
        Ok(EchoProvider)
    }
}

#[derive(Clone)]
pub(super) struct EchoProvider;

impl ThreadAware for EchoProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for EchoProvider {
    type Context = EchoContext;
    fn create(self, context: DriverContext) -> Result<Box<dyn Driver<Context = Self::Context>>, DriverError> {
        let registration = context
            .completion_service::<ReadinessClient>()?
            .register(context.readiness_waker().clone())?;
        Ok(Box::new(EchoDriver {
            state: Arc::new(EchoState {
                owner: thread::current().id(),
                queue: Mutex::new(PrivateQueue {
                    open: true,
                    alive: true,
                    next_token: 0,
                    active: 0,
                    completed_on: None,
                    requests: HashMap::new(),
                    completions: VecDeque::with_capacity(64),
                }),
                completed: Condvar::new(),
                registration,
            }),
            queue_ready: false,
            tasks: context.system_tasks().clone(),
            ready: context.readiness_waker().clone(),
        }))
    }
}

pub(super) struct EchoDriver {
    state: Arc<EchoState>,
    queue_ready: bool,
    tasks: SystemTasks,
    ready: Waker,
}

impl EchoDriver {
    fn active(&self) -> usize {
        self.state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock")
            .active
    }
}

impl Driver for EchoDriver {
    type Context = EchoContext;

    fn context(&self) -> Self::Context {
        EchoContext {
            state: Arc::clone(&self.state),
        }
    }

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        debug_assert_eq!(thread::current().id(), self.state.owner);
        self.queue_ready |= self.state.registration.take_ready();
        if !self.queue_ready {
            return Ok(ServiceStatus::Idle);
        }
        let mut queue = self
            .state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock");
        while !queue.completions.is_empty() {
            if !budget.try_consume() {
                // A single collected edge can cover many private entries. Retain continuation
                // locally; do not depend on the collector delivering a second edge.
                return Ok(ServiceStatus::Runnable);
            }
            let token = queue
                .completions
                .pop_front()
                .expect("only this owner removes from the nonempty private queue");
            let request = queue
                .requests
                .get_mut(&token)
                .ok_or_else(|| DriverError::from_message("echo completion does not name an active text request"))?;
            request.result = Some(request.input.to_ascii_uppercase());
            if !request.interested {
                queue.requests.remove(&token);
            }
            queue.active -= 1;
            queue.completed_on = Some(thread::current().id());
            self.state.completed.notify_all();
        }
        self.queue_ready = false;
        Ok(ServiceStatus::Idle)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        // Collector-to-driver notifications stay enabled. A private entry is actionable only
        // after collection, or while continuing a previously collected readiness batch.
        Ok(if self.state.registration.is_ready() || self.queue_ready {
            WaitStatus::WorkReady
        } else {
            WaitStatus::Armed
        })
    }

    fn shutdown(self: Box<Self>) -> Box<dyn Drain> {
        self.state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock")
            .open = false;
        let cleanup = Cleanup::start(&self.tasks, self.ready.clone());
        Box::new(EchoDrain { driver: self, cleanup })
    }
}

impl Drop for EchoDriver {
    fn drop(&mut self) {
        {
            let mut queue = self
                .state
                .queue
                .lock()
                .expect("a panicking echo operation poisoned its admission lock");
            queue.open = false;
            queue.alive = false;
        }
        self.state.registration.retire();
        self.state.completed.notify_all();
    }
}

struct EchoDrain {
    driver: Box<EchoDriver>,
    cleanup: Cleanup,
}

impl Drain for EchoDrain {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        let cleanup_complete = self.cleanup.check_complete()?;
        let status = self.driver.service(budget)?;
        if self.driver.active() == 0 && cleanup_complete {
            return Ok(if budget.try_consume() {
                DrainStatus::Complete
            } else {
                DrainStatus::Pending(ServiceStatus::Runnable)
            });
        }
        Ok(DrainStatus::Pending(status))
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        let cleanup_complete = self.cleanup.check_complete()?;
        if self.driver.active() == 0 && cleanup_complete {
            return Ok(WaitStatus::WorkReady);
        }
        self.driver.prepare_wait()
    }
}

#[derive(Debug)]
pub(super) struct EchoOperation {
    state: Arc<EchoState>,
    token: u64,
}

impl EchoOperation {
    pub(super) fn wait(self) -> Result<String, EchoIoError> {
        let mut queue = self
            .state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock");
        loop {
            let request = queue
                .requests
                .get_mut(&self.token)
                .expect("an echo operation retains its private request until completion");
            if let Some(result) = request.result.take() {
                queue.requests.remove(&self.token);
                return Ok(result);
            }
            if !queue.alive {
                queue.requests.remove(&self.token);
                return Err(EchoIoError::Abandoned);
            }
            queue = self
                .state
                .completed
                .wait(queue)
                .expect("a panicking echo operation poisoned its admission lock");
        }
    }

    #[cfg(test)]
    pub(super) fn is_pending(&self) -> bool {
        self.state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock")
            .requests
            .get(&self.token)
            .is_some_and(|request| request.result.is_none())
    }
}

impl Drop for EchoOperation {
    fn drop(&mut self) {
        let mut queue = self
            .state
            .queue
            .lock()
            .expect("a panicking echo operation poisoned its admission lock");
        let alive = queue.alive;
        if let Some(request) = queue.requests.get_mut(&self.token) {
            if request.result.is_some() || !alive {
                queue.requests.remove(&self.token);
            } else {
                request.interested = false;
            }
        }
    }
}

#[derive(Debug)]
pub(super) enum EchoIoError {
    Closed,
    TooLong,
    Abandoned,
    Native(DriverError),
}

impl fmt::Display for EchoIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("echo I/O admission is closed"),
            Self::TooLong => write!(f, "echo input exceeds {MAX_TEXT_BYTES} bytes"),
            Self::Abandoned => f.write_str("echo I/O was abandoned before completion"),
            Self::Native(error) => error.fmt(f),
        }
    }
}

impl Error for EchoIoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Native(error) => Some(error),
            _ => None,
        }
    }
}
