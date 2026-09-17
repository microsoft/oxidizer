// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Record-delivery driver. Only this module interprets its requests and native record words.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::task::Waker;
use std::thread::{self, ThreadId};

use arty_io_core::{
    CompletionBudget, Drain, DrainStatus, Driver, DriverContext, DriverError, DriverProvider, IoContext, LocalDriver, ServiceStatus,
    SystemTasks, WaitStatus,
};
use thread_aware_core::{Thread, ThreadAware};

use super::native::{NativeRecord, RecordClient, RecordRegistration};
use super::system_tasks::Cleanup;

#[derive(Debug)]
struct Request {
    result: Option<Result<usize, SampleIoError>>,
    interested: bool,
}

#[derive(Debug)]
struct Admission {
    open: bool,
    alive: bool,
    next_token: u64,
    active: usize,
    completed: usize,
    completed_on: Option<ThreadId>,
    requests: HashMap<u64, Request>,
}

#[derive(Debug)]
struct SampleState {
    owner: ThreadId,
    requests: Mutex<Admission>,
    completed: Condvar,
    registration: RecordRegistration,
}

#[derive(Clone, Debug)]
pub(super) struct SampleContext {
    state: Arc<SampleState>,
}

impl SampleContext {
    pub(super) fn driver_thread(&self) -> ThreadId {
        self.state.owner
    }

    pub(super) fn submit(&self, input: usize) -> Result<SampleOperation, SampleIoError> {
        let mut requests = self
            .state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock");
        if !requests.open {
            return Err(SampleIoError::Closed);
        }
        let token = requests.next_token;
        requests.next_token = token
            .checked_add(1)
            .ok_or_else(|| SampleIoError::Native(DriverError::from_message("sample request identifiers are exhausted")))?;
        requests.requests.insert(
            token,
            Request {
                result: None,
                interested: true,
            },
        );
        // Admission and active ownership use this one lock. Shutdown cannot miss an admitted
        // operation between checking admission and publishing its native-shaped packet.
        if let Err(error) = self.state.registration.post(NativeRecord { token, word: input }) {
            requests.requests.remove(&token);
            return Err(SampleIoError::Native(error));
        }
        requests.active += 1;
        Ok(SampleOperation {
            state: Arc::clone(&self.state),
            token,
        })
    }

    pub(super) fn operation_count(&self) -> usize {
        self.state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock")
            .completed
    }

    #[cfg(test)]
    pub(super) fn completed_on(&self) -> Option<ThreadId> {
        self.state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock")
            .completed_on
    }
}

impl PartialEq for SampleContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for SampleContext {}

impl ThreadAware for SampleContext {
    // Relocating the caller does not rebind this handle or move already admitted operations.
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for SampleContext {
    type Provider = SampleProvider;

    fn provider() -> Result<Self::Provider, DriverError> {
        Ok(SampleProvider)
    }
}

#[derive(Clone)]
pub(super) struct SampleProvider;

impl ThreadAware for SampleProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for SampleProvider {
    type Context = SampleContext;
    type Driver = SampleDriver;

    fn create(self, context: DriverContext) -> Result<(Self::Context, LocalDriver<Self::Driver>), DriverError> {
        let registration = context
            .completion_service::<RecordClient>()?
            .register(context.readiness_waker().clone())?;
        // One creation builds the shared state, so the published context and the installed
        // driver are the same instance's two handles.
        let state = Arc::new(SampleState {
            owner: thread::current().id(),
            requests: Mutex::new(Admission {
                open: true,
                alive: true,
                next_token: 0,
                active: 0,
                completed: 0,
                completed_on: None,
                requests: HashMap::new(),
            }),
            completed: Condvar::new(),
            registration,
        });
        let driver = SampleDriver {
            state: Arc::clone(&state),
            tasks: context.system_tasks().clone(),
            ready: context.readiness_waker().clone(),
        };
        Ok((SampleContext { state }, LocalDriver::new(driver)))
    }
}

pub(super) struct SampleDriver {
    state: Arc<SampleState>,
    tasks: SystemTasks,
    ready: Waker,
}

impl SampleDriver {
    fn active(&self) -> usize {
        self.state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock")
            .active
    }

    fn close(&self) {
        self.state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock")
            .open = false;
    }
}

impl Driver for SampleDriver {
    type Drain = SampleDrain;

    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        debug_assert_eq!(thread::current().id(), self.state.owner);
        while self.state.registration.has_records() {
            if !budget.try_consume() {
                return Ok(ServiceStatus::Runnable);
            }
            let record = self
                .state
                .registration
                .pop()
                .expect("only this owner removes records from the nonempty mailbox");
            let mut requests = self
                .state
                .requests
                .lock()
                .expect("a panicking sample operation poisoned its admission lock");
            let request = requests
                .requests
                .get_mut(&record.token)
                .ok_or_else(|| DriverError::from_message("sample record does not name an active request"))?;
            if request.result.is_some() {
                return Err(DriverError::from_message("sample request received a duplicate completion"));
            }
            request.result = Some(record.word.checked_add(1).ok_or(SampleIoError::Overflow));
            if !request.interested {
                requests.requests.remove(&record.token);
            }
            requests.active -= 1;
            requests.completed += 1;
            requests.completed_on = Some(thread::current().id());
            drop(requests);
            self.state.completed.notify_all();
        }
        Ok(ServiceStatus::Idle)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        // Mailbox notifications are permanently enabled; this is a bounded arm/recheck.
        Ok(if self.state.registration.has_records() {
            WaitStatus::WorkReady
        } else {
            WaitStatus::Armed
        })
    }

    fn shutdown(self) -> Self::Drain {
        self.close();
        let cleanup = Cleanup::start(&self.tasks, self.ready.clone());
        SampleDrain { driver: self, cleanup }
    }
}

impl Drop for SampleDriver {
    fn drop(&mut self) {
        {
            let mut requests = self
                .state
                .requests
                .lock()
                .expect("a panicking sample operation poisoned its admission lock");
            requests.open = false;
            requests.alive = false;
        }
        self.state.registration.retire();
        self.state.completed.notify_all();
        // Outstanding operations, contexts, and packets retain their own Arc-backed storage.
        // No callback follows a pointer into this driver, even on failed or abandoned draining.
    }
}

pub(super) struct SampleDrain {
    driver: SampleDriver,
    cleanup: Cleanup,
}

impl Drain for SampleDrain {
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

/// A bound operation; there is deliberately no `ThreadAware` implementation that could rebind it.
#[derive(Debug)]
pub(super) struct SampleOperation {
    state: Arc<SampleState>,
    token: u64,
}

impl SampleOperation {
    /// Blocking convenience for the example's external caller, never used on an owner worker.
    pub(super) fn wait(self) -> Result<usize, SampleIoError> {
        let mut requests = self
            .state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock");
        loop {
            let request = requests
                .requests
                .get_mut(&self.token)
                .expect("an operation retains its request until its result is taken");
            if let Some(result) = request.result.take() {
                requests.requests.remove(&self.token);
                return result;
            }
            if !requests.alive {
                requests.requests.remove(&self.token);
                return Err(SampleIoError::Abandoned);
            }
            requests = self
                .state
                .completed
                .wait(requests)
                .expect("a panicking sample operation poisoned its admission lock");
        }
    }

    #[cfg(test)]
    pub(super) fn is_pending(&self) -> bool {
        self.state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock")
            .requests
            .get(&self.token)
            .is_some_and(|request| request.result.is_none())
    }
}

impl Drop for SampleOperation {
    fn drop(&mut self) {
        let mut requests = self
            .state
            .requests
            .lock()
            .expect("a panicking sample operation poisoned its admission lock");
        let alive = requests.alive;
        if let Some(request) = requests.requests.get_mut(&self.token) {
            if request.result.is_some() || !alive {
                requests.requests.remove(&self.token);
            } else {
                // Losing interest is not cancellation: the driver still owns the active request.
                request.interested = false;
            }
        }
    }
}

#[derive(Debug)]
pub(super) enum SampleIoError {
    Closed,
    Overflow,
    Abandoned,
    Native(DriverError),
}

impl fmt::Display for SampleIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("sample I/O admission is closed"),
            Self::Overflow => f.write_str("the sample result overflows usize"),
            Self::Abandoned => f.write_str("sample I/O was abandoned before completion"),
            Self::Native(error) => error.fmt(f),
        }
    }
}

impl Error for SampleIoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Native(error) => Some(error),
            _ => None,
        }
    }
}
