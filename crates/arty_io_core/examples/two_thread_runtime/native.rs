// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe, in-memory native-adapter simulation, NOT an IOCP or `io_uring` implementation.
//!
//! A record packet names its retained mailbox before the driver decodes its words. A readiness
//! packet names a retained readiness registration; the adapter never sees that driver's private
//! completion queue. Neither path allocates a type-erased object per completion.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Wake, Waker};
use std::thread::{self, ThreadId};
use std::time::Duration;

use arty_io_core::{CompletionBudget, CompletionWaiter, DriverContext, DriverError, ServiceStatus};

const BATCH_SIZE: usize = 64;

#[derive(Debug, Default)]
struct Latch {
    pending: Mutex<bool>,
    changed: Condvar,
}

impl Latch {
    fn consume(&self) {
        *self.pending.lock().expect("only a panicking notifier can poison the wake latch") = false;
    }

    fn wait(&self, max_wait: Duration) {
        let mut pending = self.pending.lock().expect("only a panicking notifier can poison the wake latch");
        if !*pending {
            pending = if max_wait == Duration::MAX {
                self.changed
                    .wait_while(pending, |pending| !*pending)
                    .expect("only a panicking notifier can poison the wake latch")
            } else {
                // Returning early is permitted. Capping a native wait avoids duration overflow
                // without ever turning an excessively large finite timeout into an infinite one.
                self.changed
                    .wait_timeout_while(pending, max_wait.min(Duration::from_mins(1)), |pending| !*pending)
                    .expect("only a panicking notifier can poison the wake latch")
                    .0
            };
        }
        *pending = false;
    }
}

impl Wake for Latch {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        *self.pending.lock().expect("only a panicking notifier can poison the wake latch") = true;
        self.changed.notify_one();
    }
}

#[derive(Debug)]
pub(super) struct NativeMetrics {
    pub(super) owner: ThreadId,
    pub(super) records_created: AtomicUsize,
    pub(super) records_retired: AtomicUsize,
    pub(super) readiness_created: AtomicUsize,
    pub(super) readiness_retired: AtomicUsize,
}

#[derive(Debug)]
struct QueueState {
    open: bool,
    events: VecDeque<Event>,
}

#[derive(Debug)]
struct EventQueue {
    state: Mutex<QueueState>,
    latch: Arc<Latch>,
}

impl EventQueue {
    fn publish(&self, event: Event) -> Result<(), DriverError> {
        {
            let mut state = self.state.lock().expect("a panicking native producer poisoned its event queue");
            if !state.open {
                return Err(DriverError::from_message("the in-memory collector has stopped"));
            }
            state.events.push_back(event);
        }
        // Publish under the queue lock, release it, then interrupt the collector.
        self.latch.wake_by_ref();
        Ok(())
    }

    fn has_events(&self) -> bool {
        !self
            .state
            .lock()
            .expect("a panicking native producer poisoned its event queue")
            .events
            .is_empty()
    }

    fn check_open(&self) -> Result<(), DriverError> {
        if self
            .state
            .lock()
            .expect("a panicking native producer poisoned its event queue")
            .open
        {
            Ok(())
        } else {
            Err(DriverError::from_message("cannot register with a stopped in-memory collector"))
        }
    }
}

#[derive(Debug)]
enum Event {
    Record { route: Arc<RecordRoute>, record: NativeRecord },
    Ready(Arc<ReadyRoute>),
}

/// Native-shaped words, not a driver request, buffer, or result object.
#[derive(Clone, Copy, Debug)]
pub(super) struct NativeRecord {
    pub(super) token: u64,
    pub(super) word: usize,
}

#[derive(Debug)]
struct Mailbox {
    open: bool,
    records: VecDeque<NativeRecord>,
}

#[derive(Debug)]
struct RecordRoute {
    mailbox: Mutex<Mailbox>,
    ready: Waker,
    metrics: Arc<NativeMetrics>,
}

impl RecordRoute {
    fn deliver(&self, record: NativeRecord) {
        {
            let mut mailbox = self.mailbox.lock().expect("only a panicking mailbox owner can poison its mutex");
            if !mailbox.open {
                return;
            }
            mailbox.records.push_back(record);
        }
        self.ready.wake_by_ref();
    }
}

#[derive(Clone, Debug)]
pub(super) struct RecordClient {
    queue: Arc<EventQueue>,
    metrics: Arc<NativeMetrics>,
    #[cfg(test)]
    fail_next_registration: Arc<AtomicBool>,
}

impl RecordClient {
    pub(super) fn register(&self, ready: Waker) -> Result<RecordRegistration, DriverError> {
        self.queue.check_open()?;
        #[cfg(test)]
        // This switch only injects an instance-scoped creation failure; it publishes no data.
        if self.fail_next_registration.swap(false, Ordering::Relaxed) {
            return Err(DriverError::from_message("injected record registration failure"));
        }
        // Diagnostic counts do not synchronize any registration or operation state.
        self.metrics.records_created.fetch_add(1, Ordering::Relaxed);
        Ok(RecordRegistration {
            queue: Arc::clone(&self.queue),
            route: Arc::new(RecordRoute {
                mailbox: Mutex::new(Mailbox {
                    open: true,
                    records: VecDeque::with_capacity(BATCH_SIZE),
                }),
                ready,
                metrics: Arc::clone(&self.metrics),
            }),
        })
    }
}

/// A unique, non-cloneable owner of one native record route.
///
/// Dropping this handle retires the route, so a registration acquired before a later
/// construction failure cannot survive as an open mailbox. Retirement stays idempotent:
/// an explicit [`retire`](Self::retire) followed by this drop counts exactly once.
#[derive(Debug)]
pub(super) struct RecordRegistration {
    queue: Arc<EventQueue>,
    route: Arc<RecordRoute>,
}

impl RecordRegistration {
    pub(super) fn post(&self, record: NativeRecord) -> Result<(), DriverError> {
        self.queue.publish(Event::Record {
            route: Arc::clone(&self.route),
            record,
        })
    }

    pub(super) fn pop(&self) -> Option<NativeRecord> {
        self.route
            .mailbox
            .lock()
            .expect("only a panicking mailbox owner can poison its mutex")
            .records
            .pop_front()
    }

    pub(super) fn has_records(&self) -> bool {
        !self
            .route
            .mailbox
            .lock()
            .expect("only a panicking mailbox owner can poison its mutex")
            .records
            .is_empty()
    }

    pub(super) fn retire(&self) {
        let mut mailbox = self
            .route
            .mailbox
            .lock()
            .expect("only a panicking mailbox owner can poison its mutex");
        if mailbox.open {
            mailbox.open = false;
            // Diagnostic counts do not synchronize retirement.
            self.route.metrics.records_retired.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for RecordRegistration {
    fn drop(&mut self) {
        // Retirement closes the route without invalidating it: in-flight packets keep their own
        // reference and are discarded by the closed mailbox instead of landing in an orphan.
        self.retire();
    }
}

#[derive(Debug)]
struct ReadyRoute {
    pending: AtomicBool,
    retired: AtomicBool,
    ready: Waker,
    metrics: Arc<NativeMetrics>,
}

impl ReadyRoute {
    fn deliver(&self) {
        // Acquire observes retirement; late packets retain this old route, never a reused slot.
        if !self.retired.load(Ordering::Acquire) {
            // Release publishes private-queue readiness before notifying the service source.
            self.pending.store(true, Ordering::Release);
            self.ready.wake_by_ref();
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ReadinessClient {
    queue: Arc<EventQueue>,
    metrics: Arc<NativeMetrics>,
}

impl ReadinessClient {
    pub(super) fn register(&self, ready: Waker) -> Result<ReadinessRegistration, DriverError> {
        self.queue.check_open()?;
        // Diagnostic counts do not synchronize any registration or operation state.
        self.metrics.readiness_created.fetch_add(1, Ordering::Relaxed);
        Ok(ReadinessRegistration {
            queue: Arc::clone(&self.queue),
            route: Arc::new(ReadyRoute {
                pending: AtomicBool::new(false),
                retired: AtomicBool::new(false),
                ready,
                metrics: Arc::clone(&self.metrics),
            }),
        })
    }
}

/// A unique, non-cloneable owner of one native readiness route.
///
/// Dropping this handle retires the route, so a registration acquired before a later
/// construction failure cannot survive as a live notification target. Retirement stays
/// idempotent: an explicit [`retire`](Self::retire) followed by this drop counts exactly once.
#[derive(Debug)]
pub(super) struct ReadinessRegistration {
    queue: Arc<EventQueue>,
    route: Arc<ReadyRoute>,
}

impl ReadinessRegistration {
    pub(super) fn post(&self) -> Result<(), DriverError> {
        self.queue.publish(Event::Ready(Arc::clone(&self.route)))
    }

    pub(super) fn take_ready(&self) -> bool {
        // Acquire observes collector publication; clearing BEFORE service preserves a later edge.
        self.route.pending.swap(false, Ordering::AcqRel)
    }

    pub(super) fn is_ready(&self) -> bool {
        // Acquire pairs with the collector's release publication.
        self.route.pending.load(Ordering::Acquire)
    }

    pub(super) fn retire(&self) {
        // Release closes this route, and acquire makes retirement counting exactly once.
        if !self.route.retired.swap(true, Ordering::AcqRel) {
            // Diagnostic counts do not synchronize retirement.
            self.route.metrics.readiness_retired.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for ReadinessRegistration {
    fn drop(&mut self) {
        // A late packet retains this closed route and is discarded, never redirected to a
        // replacement registration.
        self.retire();
    }
}

pub(super) struct NativeWaiter {
    queue: Arc<EventQueue>,
    batch: Vec<Event>,
    metrics: Arc<NativeMetrics>,
    #[cfg(test)]
    fail_record_registration: Arc<AtomicBool>,
    #[cfg(test)]
    record_client_enabled: AtomicBool,
    #[cfg(test)]
    before_wait: Option<Box<dyn FnOnce()>>,
    #[cfg(test)]
    wait_entries: usize,
}

impl NativeWaiter {
    pub(super) fn new() -> Self {
        Self {
            queue: Arc::new(EventQueue {
                state: Mutex::new(QueueState {
                    open: true,
                    events: VecDeque::with_capacity(BATCH_SIZE),
                }),
                latch: Arc::default(),
            }),
            batch: Vec::with_capacity(BATCH_SIZE),
            metrics: Arc::new(NativeMetrics {
                owner: thread::current().id(),
                records_created: AtomicUsize::new(0),
                records_retired: AtomicUsize::new(0),
                readiness_created: AtomicUsize::new(0),
                readiness_retired: AtomicUsize::new(0),
            }),
            #[cfg(test)]
            fail_record_registration: Arc::default(),
            #[cfg(test)]
            record_client_enabled: AtomicBool::new(true),
            #[cfg(test)]
            before_wait: None,
            #[cfg(test)]
            wait_entries: 0,
        }
    }

    pub(super) fn record_client(&self) -> RecordClient {
        RecordClient {
            queue: Arc::clone(&self.queue),
            metrics: Arc::clone(&self.metrics),
            #[cfg(test)]
            fail_next_registration: Arc::clone(&self.fail_record_registration),
        }
    }

    pub(super) fn readiness_client(&self) -> ReadinessClient {
        ReadinessClient {
            queue: Arc::clone(&self.queue),
            metrics: Arc::clone(&self.metrics),
        }
    }

    #[cfg(test)]
    pub(super) fn metrics(&self) -> Arc<NativeMetrics> {
        Arc::clone(&self.metrics)
    }

    #[cfg(test)]
    pub(super) fn fail_next_record_registration(&self) {
        // This test-only switch is configured before clients are handed to the owner loop.
        self.fail_record_registration.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn disable_record_client(&self) {
        self.record_client_enabled.store(false, Ordering::Relaxed);
    }

    fn status(&self) -> ServiceStatus {
        if self.queue.has_events() {
            ServiceStatus::Runnable
        } else {
            ServiceStatus::Idle
        }
    }
}

impl CompletionWaiter for NativeWaiter {
    fn attach_clients(&self, context: DriverContext) -> Result<DriverContext, DriverError> {
        #[cfg(test)]
        if !self.record_client_enabled.load(Ordering::Relaxed) {
            return context.with_completion_service(self.readiness_client());
        }
        context
            .with_completion_service(self.record_client())?
            .with_completion_service(self.readiness_client())
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.queue.latch))
    }

    fn collect(&mut self, max_wait: Duration, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        debug_assert_eq!(thread::current().id(), self.metrics.owner);
        if budget.remaining() == 0 {
            return Ok(self.status());
        }
        if !max_wait.is_zero() {
            if self.queue.has_events() {
                self.queue.latch.consume();
            } else {
                #[cfg(test)]
                {
                    self.wait_entries += 1;
                }
                #[cfg(test)]
                if let Some(hook) = self.before_wait.take() {
                    hook();
                }
                // Only the private latch mutex participates in this wait, and Condvar releases it.
                self.queue.latch.wait(max_wait);
            }
        }
        {
            let mut state = self
                .queue
                .state
                .lock()
                .expect("a panicking native producer poisoned its event queue");
            let available = state.events.len().min(BATCH_SIZE);
            for _ in 0..available {
                if !budget.try_consume() {
                    break;
                }
                self.batch
                    .push(state.events.pop_front().expect("available is bounded by the locked queue length"));
            }
        }
        for event in self.batch.drain(..) {
            match event {
                Event::Record { route, record } => route.deliver(record),
                Event::Ready(route) => route.deliver(),
            }
        }
        Ok(self.status())
    }
}

impl Drop for NativeWaiter {
    fn drop(&mut self) {
        // Retained clients and packets own their storage independently of the domain identity.
        self.queue
            .state
            .lock()
            .expect("a panicking native producer poisoned its event queue")
            .open = false;
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
