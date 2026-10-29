// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread::ThreadId;

use mio::event::Event;
use mio::{Registry, Token};
use observed::Sink;
use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::Mutex;

use crate::registration::RegistrationEntry;
use crate::telemetry::{self, SourceOperation};

pub(crate) const DRIVER_WAKE_TOKEN: Token = Token(usize::MAX);

pub(crate) struct ContextState {
    workers: Mutex<HashMap<ThreadId, Arc<WorkerState>>>,
    sink: Sink,
    attachment_failure_reported: AtomicBool,
}

impl ContextState {
    pub(crate) fn new(sink: Sink) -> Self {
        Self {
            workers: Mutex::new(HashMap::new()),
            sink,
            attachment_failure_reported: AtomicBool::new(false),
        }
    }

    pub(crate) const fn sink(&self) -> &Sink {
        &self.sink
    }

    pub(crate) fn report_attachment_failure(&self, error: &io::Error) {
        if self
            .attachment_failure_reported
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            telemetry::report_context_attachment_failed(&self.sink, error);
        }
    }

    pub(crate) fn worker(&self, thread_id: ThreadId) -> Option<Arc<WorkerState>> {
        self.workers.lock().get(&thread_id).cloned()
    }

    pub(crate) fn try_worker(&self, thread_id: ThreadId) -> Option<Arc<WorkerState>> {
        self.workers.try_lock_result().ok().flatten()?.get(&thread_id).cloned()
    }

    pub(crate) fn install(&self, worker: Arc<WorkerState>) -> io::Result<()> {
        let mut workers = self.workers.lock();
        if workers.contains_key(&worker.thread_id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the current thread already has an Arty Mio driver",
            ));
        }
        workers.insert(worker.thread_id, worker);
        Ok(())
    }

    pub(crate) fn remove(&self, worker: &Arc<WorkerState>) {
        let mut workers = self.workers.lock();
        if workers.get(&worker.thread_id).is_some_and(|current| Arc::ptr_eq(current, worker)) {
            workers.remove(&worker.thread_id);
        }
    }
}

pub(crate) struct WorkerState {
    pub(crate) thread_id: ThreadId,
    owner_waker: Waker,
    next_token: AtomicUsize,
    registry: Registry,
    registrations: Mutex<Registrations>,
    registrations_idle: Condvar,
    failure: Mutex<Option<io::Error>>,
    sink: Sink,
    registration_failure_reported: AtomicBool,
    deregistration_failure_reported: AtomicBool,
}

#[derive(Default)]
struct Registrations {
    closed: bool,
    active_operations: usize,
    values: HashMap<Token, Arc<RegistrationEntry>>,
}

impl WorkerState {
    pub(crate) fn new(thread_id: ThreadId, registry: Registry, owner_waker: Waker, sink: Sink) -> Self {
        Self {
            thread_id,
            owner_waker,
            next_token: AtomicUsize::new(0),
            registry,
            registrations: Mutex::new(Registrations::default()),
            registrations_idle: Condvar::new(),
            failure: Mutex::new(None),
            sink,
            registration_failure_reported: AtomicBool::new(false),
            deregistration_failure_reported: AtomicBool::new(false),
        }
    }

    pub(crate) const fn registry(&self) -> &Registry {
        &self.registry
    }

    pub(crate) const fn sink(&self) -> &Sink {
        &self.sink
    }

    pub(crate) fn allocate_token(&self) -> io::Result<Token> {
        self.next_token
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
                (token < usize::MAX - 1).then_some(token + 1)
            })
            .map(Token)
            .map_err(|_exhausted| io::Error::other("the Arty Mio token space is exhausted"))
    }

    pub(crate) fn begin_registration(&self, token: Token, entry: Arc<RegistrationEntry>) -> io::Result<()> {
        let mut registrations = self.registrations.lock();
        if registrations.closed {
            return Err(closed_error());
        }
        registrations.active_operations += 1;
        registrations.values.insert(token, entry);
        Ok(())
    }

    pub(crate) fn finish_registration(&self, token: Token, entry: &Arc<RegistrationEntry>, registered: bool) -> bool {
        let mut registrations = self.registrations.lock();
        let active =
            registered && !registrations.closed && registrations.values.get(&token).is_some_and(|current| Arc::ptr_eq(current, entry));
        if !active {
            registrations.values.remove(&token);
        }
        active
    }

    pub(crate) fn begin_deregistration(&self, token: Token, entry: &Arc<RegistrationEntry>) -> bool {
        let mut registrations = self.registrations.lock();
        if registrations.closed || !registrations.values.get(&token).is_some_and(|current| Arc::ptr_eq(current, entry)) {
            return false;
        }
        registrations.active_operations += 1;
        registrations.values.remove(&token);
        true
    }

    pub(crate) fn finish_registry_operation(&self) {
        let mut registrations = self.registrations.lock();
        registrations.active_operations -= 1;
        if registrations.active_operations == 0 {
            self.registrations_idle.notify_all();
        }
    }

    pub(crate) fn dispatch_batch(&self, events: &[Event], resolved: &mut Vec<Option<Arc<RegistrationEntry>>>) {
        resolved.clear();
        if events.is_empty() {
            return;
        }
        let registrations = self.registrations.lock();
        resolved.extend(events.iter().map(|event| registrations.values.get(&event.token()).cloned()));
        drop(registrations);
        for (event, registration) in events.iter().zip(resolved.iter()) {
            if let Some(registration) = registration {
                registration.notify(event);
            }
        }
    }

    pub(crate) fn close_registrations(&self) {
        let registrations: Vec<_> = {
            let mut registrations = self.registrations.lock();
            registrations.closed = true;
            while registrations.active_operations != 0 {
                registrations = self.registrations_idle.wait(registrations);
            }
            registrations.values.drain().map(|(_, registration)| registration).collect()
        };
        for registration in registrations {
            registration.close();
        }
    }

    #[cfg(test)]
    pub(crate) fn registrations_are_closed(&self) -> bool {
        self.registrations.lock().closed
    }

    pub(crate) fn wake_owner(&self) {
        self.owner_waker.wake_by_ref();
    }

    pub(crate) fn record_failure(&self, error: io::Error) {
        let mut failure = self.failure.lock();
        if failure.is_none() {
            *failure = Some(error);
            drop(failure);
            self.owner_waker.wake_by_ref();
        }
    }

    pub(crate) fn take_failure(&self) -> Option<io::Error> {
        self.failure.lock().take()
    }

    pub(crate) fn report_source_failure(&self, operation: SourceOperation, error: &io::Error) {
        let reported = match operation {
            SourceOperation::Registration => &self.registration_failure_reported,
            SourceOperation::Deregistration => &self.deregistration_failure_reported,
        };
        if reported.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            telemetry::report_source_operation_failed(&self.sink, operation, error);
        }
    }
}

fn closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the Arty Mio driver is closed")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::Ordering;
    use std::task::Waker;

    use mio::{Poll, Token};
    use observed::Sink;

    use super::WorkerState;

    #[test]
    fn token_allocation_stops_before_the_reserved_waker_token() {
        let poll = Poll::new().unwrap();
        let worker = WorkerState::new(
            std::thread::current().id(),
            poll.registry().try_clone().unwrap(),
            Waker::noop().clone(),
            Sink::noop(),
        );
        worker.next_token.store(usize::MAX - 2, Ordering::Relaxed);

        assert_eq!(worker.allocate_token().unwrap(), Token(usize::MAX - 2));
        assert_eq!(worker.allocate_token().unwrap_err().kind(), std::io::ErrorKind::Other);
    }
}
