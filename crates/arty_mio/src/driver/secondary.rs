// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc as StdArc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};

use arty_io_core::DriverError;
use mio::event::Event;
use mio::{Events, Poll, Registry};
use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::{Mutex, MutexGuard};

use super::poll_wake::PollWake;
use super::{COMPLETION_BUDGET, EVENT_CAPACITY, SHUTDOWN_LIMIT};
use crate::registration::RegistrationEntry;
use crate::worker::{DRIVER_WAKE_TOKEN, WorkerState};

pub(super) struct SecondaryDriver {
    pub(super) shared: Arc<SecondaryShared>,
    finished: Receiver<()>,
    thread: JoinHandle<()>,
    events: Vec<Event>,
    resolved: Vec<Option<Arc<RegistrationEntry>>>,
}

impl SecondaryDriver {
    pub(super) fn new() -> io::Result<(Self, StdArc<PollWake>, Registry)> {
        let shared = Arc::new(SecondaryShared::default());
        let thread_shared = Arc::clone(&shared);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (finished_tx, finished) = mpsc::channel();
        let thread = thread::Builder::new().name("arty-mio-secondary".to_owned()).spawn(move || {
            let result = (|| -> io::Result<()> {
                let mut poll = Poll::new()?;
                let poll_waker = StdArc::new(PollWake::new(poll.registry())?);
                let registry = poll.registry().try_clone()?;
                started_tx
                    .send(Ok((StdArc::clone(&poll_waker), registry)))
                    .map_err(|_send_error| io::Error::other("the Arty worker stopped secondary driver creation"))?;
                let mut events = Events::with_capacity(EVENT_CAPACITY);
                loop {
                    thread_shared.worker()?;
                    if thread_shared.shutdown_requested() {
                        break;
                    }

                    match poll.poll(&mut events, None) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error),
                    }

                    let mut state = thread_shared.state();
                    state
                        .events
                        .extend(events.iter().filter(|event| event.token() != DRIVER_WAKE_TOKEN).cloned());
                    let has_events = !state.events.is_empty();
                    state.paused = has_events;
                    drop(state);
                    if has_events {
                        thread_shared.worker()?.wake_owner();
                        thread_shared.wait_until_resumed();
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                thread_shared.fail(error);
                if let Some(worker) = thread_shared.try_worker() {
                    worker.wake_owner();
                }
            }
            let _ = finished_tx.send(());
        })?;

        let (poll_waker, registry) = match started_rx.recv() {
            Ok(Ok(started)) => started,
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(io::Error::other("the secondary Arty Mio poll thread stopped during creation")),
        };
        Ok((
            Self {
                shared,
                finished,
                thread,
                events: Vec::with_capacity(COMPLETION_BUDGET),
                resolved: Vec::with_capacity(COMPLETION_BUDGET),
            },
            poll_waker,
            registry,
        ))
    }

    pub(super) fn attach(&self, worker: Arc<WorkerState>) {
        self.shared.set_worker(worker);
    }

    pub(super) fn execute_cycle(&mut self, worker: &WorkerState) -> Result<(), DriverError> {
        let (failure, resume, has_events) = drain_secondary_events(&self.shared, &mut self.events);
        if resume {
            self.shared.ready.notify_one();
        }
        if let Some(error) = failure {
            return Err(DriverError::from_source(error));
        }
        if let Some(error) = worker.take_failure() {
            return Err(DriverError::from_source(error));
        }
        worker.dispatch_batch(&self.events, &mut self.resolved);
        if has_events {
            worker.wake_owner();
        }
        Ok(())
    }

    pub(super) fn shutdown(self, poll_waker: &PollWake, worker: &WorkerState) -> Result<(), DriverError> {
        self.shared.set_shutdown();
        poll_waker.wake_poll();
        let result = match self.finished.recv_timeout(SHUTDOWN_LIMIT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => self
                .thread
                .join()
                .map_err(|_panic| DriverError::from_message("the secondary Arty Mio poll thread panicked")),
            Err(RecvTimeoutError::Timeout) => Err(DriverError::from_message(
                "the secondary Arty Mio poll thread did not stop within one second",
            )),
        };
        result?;
        if let Some(error) = worker.take_failure() {
            return Err(DriverError::from_source(error));
        }
        match self.shared.take_failure() {
            Some(error) => Err(DriverError::from_source(error)),
            None => Ok(()),
        }
    }
}

pub(super) fn drain_secondary_events(shared: &SecondaryShared, events: &mut Vec<Event>) -> (Option<io::Error>, bool, bool) {
    events.clear();
    let mut state = shared.state();
    let count = COMPLETION_BUDGET.min(state.events.len());
    events.extend(state.events.drain(..count));
    let failure = state.failure.take();
    let has_events = !state.events.is_empty();
    let resume = !has_events;
    if resume {
        state.paused = false;
    }
    (failure, resume, has_events)
}

#[derive(Default)]
pub(super) struct SecondaryShared {
    inner: Mutex<SecondaryState>,
    ready: Condvar,
}

impl SecondaryShared {
    pub(super) fn state(&self) -> MutexGuard<'_, SecondaryState> {
        self.inner.lock()
    }

    fn set_worker(&self, worker: Arc<WorkerState>) {
        let mut state = self.state();
        state.worker = Some(worker);
        self.ready.notify_all();
    }

    fn worker(&self) -> io::Result<Arc<WorkerState>> {
        let mut state = self.state();
        while state.worker.is_none() && !state.shutdown {
            state = self.ready.wait(state);
        }
        state
            .worker
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "the secondary Arty Mio driver is closed"))
    }

    fn try_worker(&self) -> Option<Arc<WorkerState>> {
        self.state().worker.clone()
    }

    fn wait_until_resumed(&self) {
        let mut state = self.state();
        while state.paused && !state.shutdown {
            state = self.ready.wait(state);
        }
    }

    pub(super) fn fail(&self, error: io::Error) {
        let mut state = self.state();
        if state.failure.is_none() {
            state.failure = Some(error);
        }
    }

    fn take_failure(&self) -> Option<io::Error> {
        self.state().failure.take()
    }

    fn set_shutdown(&self) {
        let mut state = self.state();
        state.shutdown = true;
        self.ready.notify_all();
    }

    fn shutdown_requested(&self) -> bool {
        self.state().shutdown
    }
}

#[derive(Default)]
pub(super) struct SecondaryState {
    worker: Option<Arc<WorkerState>>,
    pub(super) events: VecDeque<Event>,
    failure: Option<io::Error>,
    pub(super) paused: bool,
    shutdown: bool,
}
