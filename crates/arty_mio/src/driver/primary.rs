// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use arty_io_core::DriverError;
use mio::event::Event;
use mio::{Events, Poll};
use performables::arc::Arc;

use super::{COMPLETION_BUDGET, EVENT_CAPACITY};
use crate::registration::RegistrationEntry;
use crate::worker::{DRIVER_WAKE_TOKEN, WorkerState};

pub(super) struct PrimaryDriver {
    poll: Poll,
    events: Events,
    pub(super) pending: VecDeque<Event>,
    batch: Vec<Event>,
    resolved: Vec<Option<Arc<RegistrationEntry>>>,
}

impl PrimaryDriver {
    pub(super) fn new(poll: Poll) -> Self {
        Self {
            poll,
            events: Events::with_capacity(EVENT_CAPACITY),
            pending: VecDeque::new(),
            batch: Vec::with_capacity(COMPLETION_BUDGET),
            resolved: Vec::with_capacity(COMPLETION_BUDGET),
        }
    }

    pub(super) fn execute_cycle(&mut self, worker: &Arc<WorkerState>, max_wait: Duration) -> Result<(), DriverError> {
        if let Some(error) = worker.take_failure() {
            return Err(DriverError::from_source(error));
        }

        let processed_pending = self.dispatch_pending(worker, COMPLETION_BUDGET);
        if !self.pending.is_empty() {
            worker.wake_owner();
            return Ok(());
        }

        let timeout = if processed_pending != 0 {
            Some(Duration::ZERO)
        } else if max_wait == Duration::MAX {
            None
        } else {
            Some(max_wait)
        };
        match self.poll.poll(&mut self.events, timeout) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(error) => return Err(DriverError::from_source(error)),
        }

        self.pending
            .extend(self.events.iter().filter(|event| event.token() != DRIVER_WAKE_TOKEN).cloned());
        self.dispatch_pending(worker, COMPLETION_BUDGET);
        if !self.pending.is_empty() || self.events.iter().count() == self.events.capacity() {
            worker.wake_owner();
        }
        Ok(())
    }

    fn dispatch_pending(&mut self, worker: &WorkerState, budget: usize) -> usize {
        let count = budget.min(self.pending.len());
        if count != 0 {
            self.batch.clear();
            self.batch.extend(self.pending.drain(..count));
            worker.dispatch_batch(&self.batch, &mut self.resolved);
        }
        count
    }
}
