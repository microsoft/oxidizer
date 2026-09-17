// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The owner-thread scheduler: one bounded turn per source, one waiter, cooperative draining.
//!
//! Newly installed drivers and newly created drains receive an initial runnable turn without
//! needing a notification. Errors from either drain service or arming retire the slot permanently.
//! This runtime chooses private boxed erasure for heterogeneous owners. The adapters forward
//! statically into concrete drivers and drains; no public driver trait object is involved.

use std::any::TypeId;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use arty_io_core::{
    CompletionBudget, CompletionWaiter, Drain, DrainStatus, Driver, DriverError, LocalDrain, LocalDriver, ServiceStatus, WaitStatus,
};

pub(super) trait ErasedDriver {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError>;
    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError>;
    fn shutdown(self: Box<Self>) -> Box<dyn ErasedDrain>;
}

pub(super) trait ErasedDrain {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError>;
    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError>;
}

impl<D: Driver> ErasedDriver for LocalDriver<D> {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        self.service(budget)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.prepare_wait()
    }

    fn shutdown(self: Box<Self>) -> Box<dyn ErasedDrain> {
        Box::new((*self).shutdown())
    }
}

impl<S: Drain> ErasedDrain for LocalDrain<S> {
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        self.service(budget)
    }

    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.prepare_wait()
    }
}

#[derive(Debug)]
pub(super) struct Source {
    ready: AtomicBool,
    domain: Waker,
}

impl Source {
    pub(super) fn new(domain: Waker) -> Arc<Self> {
        Arc::new(Self {
            ready: AtomicBool::new(false),
            domain,
        })
    }

    pub(super) fn is_ready(&self) -> bool {
        // Acquire observes publication preceding a source notification.
        self.ready.load(Ordering::Acquire)
    }

    fn begin_turn(&self) {
        // Clear BEFORE servicing. AcqRel consumes the old notification without erasing a wake
        // that arrives while service or prepare_wait runs. There is no trailing clear.
        self.ready.swap(false, Ordering::AcqRel);
    }
}

impl Wake for Source {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // Release publishes source readiness before interrupting the separate domain latch.
        self.ready.store(true, Ordering::Release);
        self.domain.wake_by_ref();
    }
}

enum Participant {
    Driver(Box<dyn ErasedDriver>),
    Drain(Box<dyn ErasedDrain>),
}

struct Entry {
    id: TypeId,
    source: Arc<Source>,
    participant: Option<Participant>,
    status: ServiceStatus,
    errors: Vec<DriverError>,
}

impl Entry {
    fn is_runnable(&self, now: Instant) -> bool {
        self.participant.is_some() && (self.source.is_ready() || due(self.status, now))
    }

    fn begin_shutdown(&mut self) {
        if let Some(participant) = self.participant.take() {
            self.participant = Some(match participant {
                Participant::Driver(driver) => Participant::Drain(driver.shutdown()),
                Participant::Drain(drain) => Participant::Drain(drain),
            });
            self.status = ServiceStatus::Runnable;
        }
    }

    fn fail(&mut self, error: DriverError) {
        self.errors.push(error);
        if matches!(self.participant, Some(Participant::Driver(_))) {
            self.begin_shutdown();
        } else {
            // Either drain error is terminal: release it and never call either method again.
            self.participant = None;
        }
    }

    fn service(&mut self, quantum: NonZeroUsize) -> bool {
        self.source.begin_turn();
        let mut budget = CompletionBudget::new(quantum);
        let result = match self.participant.as_mut() {
            Some(Participant::Driver(driver)) => driver.service(&mut budget).map(DrainStatus::Pending),
            Some(Participant::Drain(drain)) => drain.service(&mut budget),
            None => return false,
        };
        match result {
            Ok(DrainStatus::Pending(status)) => self.status = status,
            Ok(DrainStatus::Complete) => self.participant = None,
            Err(error) => {
                self.fail(error);
                return true;
            }
        }
        false
    }

    fn arm(&mut self) -> bool {
        let result = match self.participant.as_mut() {
            Some(Participant::Driver(driver)) => driver.prepare_wait(),
            Some(Participant::Drain(drain)) => drain.prepare_wait(),
            None => return false,
        };
        match result {
            Ok(WaitStatus::Armed) => {}
            Ok(WaitStatus::WorkReady) => self.status = ServiceStatus::Runnable,
            Err(error) => {
                self.fail(error);
                return true;
            }
        }
        false
    }
}

pub(super) struct Retired {
    pub(super) id: TypeId,
    pub(super) errors: Vec<DriverError>,
}

pub(super) struct Coordinator<W> {
    pub(super) waiter: W,
    entries: Vec<Entry>,
    first: usize,
    quantum: NonZeroUsize,
    collection: ServiceStatus,
    collector_failed: bool,
    failed: bool,
    domain_errors: Vec<DriverError>,
    retired: Vec<Retired>,
}

impl<W: CompletionWaiter> Coordinator<W> {
    pub(super) fn new(waiter: W, quantum: NonZeroUsize) -> Self {
        Self {
            waiter,
            entries: Vec::new(),
            first: 0,
            quantum,
            collection: ServiceStatus::Idle,
            collector_failed: false,
            failed: false,
            domain_errors: Vec::new(),
            retired: Vec::new(),
        }
    }

    pub(super) fn insert(&mut self, id: TypeId, source: Arc<Source>, driver: Box<dyn ErasedDriver>) {
        assert!(!self.contains(id), "registration must be unique until its previous drain retires");
        self.entries.push(Entry {
            id,
            source,
            participant: Some(Participant::Driver(driver)),
            status: ServiceStatus::Runnable,
            errors: Vec::new(),
        });
    }

    pub(super) fn contains(&self, id: TypeId) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    pub(super) fn service(&mut self, now: Instant) {
        // Collection continues under sustained runnable driver, control, or drain work.
        self.collect(Duration::ZERO);
        let count = self.entries.len();
        for offset in 0..count {
            let index = (self.first + offset) % count;
            let entry = &mut self.entries[index];
            if entry.is_runnable(now) {
                self.failed |= entry.service(self.quantum);
            }
        }
        self.first = if count == 0 { 0 } else { (self.first + 1) % count };
        self.reap();
    }

    pub(super) fn arm(&mut self) {
        // Arm every participant even when an earlier participant discovers work.
        for entry in &mut self.entries {
            self.failed |= entry.arm();
        }
        self.reap();
    }

    pub(super) fn collect(&mut self, wait: Duration) {
        if self.collector_failed {
            return;
        }
        let mut budget = CompletionBudget::new(self.quantum);
        match self.waiter.collect(wait, &mut budget) {
            Ok(status) => self.collection = status,
            Err(error) => {
                self.collector_failed = true;
                self.failed = true;
                self.domain_errors.push(error);
            }
        }
    }

    pub(super) fn wait_duration(&self, now: Instant, overall_deadline: Option<Instant>) -> Duration {
        if self.collector_failed || due(self.collection, now) || self.entries.iter().any(|entry| entry.is_runnable(now)) {
            return Duration::ZERO;
        }
        let nearest = self
            .entries
            .iter()
            .filter_map(|entry| deadline(entry.status))
            .chain(deadline(self.collection))
            .chain(overall_deadline)
            .min();
        nearest.map_or(Duration::MAX, |deadline| deadline.saturating_duration_since(now))
    }

    pub(super) fn begin_shutdown(&mut self, id: TypeId) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.begin_shutdown();
            true
        } else {
            false
        }
    }

    pub(super) fn begin_shutdown_all(&mut self) {
        for entry in &mut self.entries {
            entry.begin_shutdown();
        }
    }

    pub(super) fn expire(&mut self, id: TypeId) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.errors.push(DriverError::shutdown_timeout());
            entry.participant = None;
        }
        self.reap();
    }

    pub(super) fn expire_all(&mut self) {
        for entry in &mut self.entries {
            entry.errors.push(DriverError::shutdown_timeout());
            entry.participant = None;
        }
        self.reap();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(super) fn has_failed(&self) -> bool {
        self.failed
    }

    pub(super) fn take_retired(&mut self) -> Vec<Retired> {
        std::mem::take(&mut self.retired)
    }

    pub(super) fn take_domain_errors(&mut self) -> Vec<DriverError> {
        std::mem::take(&mut self.domain_errors)
    }

    fn reap(&mut self) {
        self.entries.retain_mut(|entry| {
            if entry.participant.is_none() {
                self.retired.push(Retired {
                    id: entry.id,
                    errors: std::mem::take(&mut entry.errors),
                });
                false
            } else {
                true
            }
        });
    }
}

fn due(status: ServiceStatus, now: Instant) -> bool {
    match status {
        ServiceStatus::Idle => false,
        ServiceStatus::Runnable => true,
        ServiceStatus::Deadline(deadline) => deadline <= now,
    }
}

fn deadline(status: ServiceStatus) -> Option<Instant> {
    match status {
        ServiceStatus::Deadline(deadline) => Some(deadline),
        ServiceStatus::Idle | ServiceStatus::Runnable => None,
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
