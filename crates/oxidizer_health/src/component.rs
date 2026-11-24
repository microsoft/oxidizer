// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use arc_swap::{ArcSwap, ArcSwapOption};
use performables::sync::channel::{WatchReceiver, WatchSender, watch};
use thread_aware::{Thread, ThreadAware};

use crate::aggregator::{ProcessAggregate, next_id, wait};
use crate::report::Snapshot;
use crate::worker::{Work, Worker};
use crate::{Attribute, AttributeString, AttributeValue, Filter, Health, Report, Signal};

const REASON: AttributeString = AttributeString::new("reason");
const MAX_BATCH: usize = 256;

/// One logical application subsystem, aggregating the signals of its publishers.
///
/// A component's health is the worst signal among its active publishers. A component without
/// publishers is `Unhealthy`. Clones refer to the same logical component; each clone has an
/// independent [`changed`](Self::changed) notification cursor. The component stops and
/// unregisters from its [`Aggregator`](crate::Aggregator) when its last handle is dropped.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Health};
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let mut database = aggregator.component("database");
/// assert_eq!(database.state(), Health::Unhealthy);
///
/// let mut publisher = database.publisher();
/// while database.state() != Health::Healthy {
///     database.changed().await;
/// }
///
/// publisher.degraded("high latency");
/// while database.state() != Health::Degraded {
///     database.changed().await;
/// }
///
/// drop(publisher);
/// while database.state() != Health::Unhealthy {
///     database.changed().await;
/// }
/// # });
/// ```
pub struct Component {
    owner: Arc<ComponentOwner>,
    changes: WatchReceiver<()>,
}

impl Component {
    pub(crate) fn new(owner: Arc<ComponentOwner>) -> Self {
        let changes = owner.shared.changes.subscribe();
        Self { owner, changes }
    }

    /// Creates an independently owned publisher in `Healthy` state with no attributes.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "a live handle cannot have a retired registration")]
    pub fn publisher(&self) -> Publisher {
        Publisher::register(&self.owner.shared).expect("this handle keeps the component registered")
    }

    /// Waits until the component state or report data may have changed.
    ///
    /// Intermediate transitions can be coalesced. Read [`state`](Self::state) or
    /// [`report`](Self::report) afterwards to observe the current state.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "exclusive access keeps one waiter per notification cursor"
    )]
    pub async fn changed(&mut self) {
        wait(&self.changes).await;
    }

    /// Returns the current aggregate component state.
    #[must_use]
    pub fn state(&self) -> Health {
        if self.owner.worker.is_running() {
            self.owner.shared.snapshot.load().state()
        } else {
            Health::Unhealthy
        }
    }

    /// Returns a consistent snapshot of the component's publisher signals.
    #[expect(
        clippy::unused_async,
        reason = "the contract allows report assembly to wait for the component owner"
    )]
    pub async fn report(&self, filter: Filter) -> Report {
        self.owner.shared.report(filter)
    }
}

impl Clone for Component {
    fn clone(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
            changes: self.changes.clone(),
        }
    }
}

impl Debug for Component {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("name", &self.owner.shared.name)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

// Relocation keeps the handle to the same logical component; the shared owner is thread-safe.
impl ThreadAware for Component {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

/// Owns the logical component independently of in-flight publisher updates.
pub(crate) struct ComponentOwner {
    shared: Arc<ComponentShared>,
    worker: Arc<Worker>,
}

impl ComponentOwner {
    pub(crate) fn new(shared: Arc<ComponentShared>, worker: Arc<Worker>) -> Arc<Self> {
        Arc::new(Self { shared, worker })
    }
}

impl Drop for ComponentOwner {
    fn drop(&mut self) {
        let registration = &self.shared.dispatch.registration;
        // Keep a veto until the worker removes this exact registration. New publications have
        // no access after retirement; already admitted ones finish against the old registration.
        registration
            .load()
            .as_ref()
            .expect("only the last owner retires its registration")
            .block();
        registration.store(None);
    }
}

/// Owner of one component's aggregation state.
///
/// Publishers hand their latest signal to a per-publisher [`Slot`] and enqueue the slot at most
/// once while it is pending. The shared worker applies queued slots to the actor state and
/// publishes snapshots.
pub(crate) struct ComponentShared {
    name: Arc<str>,
    id: u64,
    next_publisher: AtomicU64,
    dispatch: Arc<ComponentDispatch>,
    actor: Mutex<ComponentActor>,
    snapshot: ArcSwap<Snapshot>,
    changes: WatchSender<()>,
}

struct ComponentActor {
    updates: Receiver<Arc<Slot>>,
    signals: HashMap<u64, Arc<Signal>>,
    dirty: bool,
}

impl ComponentShared {
    pub(crate) fn new(name: Arc<str>, id: u64, worker: Sender<Work>, process_blockers: Arc<AtomicUsize>) -> Arc<Self> {
        let (updates, receiver) = mpsc::channel();
        let (changes, _) = watch(());
        // Register the empty-component veto before any publisher can obtain the component.
        process_blockers.fetch_add(1, Ordering::SeqCst);
        Arc::new_cyclic(|component| Self {
            name,
            id,
            next_publisher: AtomicU64::default(),
            dispatch: Arc::new(ComponentDispatch {
                component: Weak::clone(component),
                updates,
                registration: ArcSwapOption::from_pointee(Registration {
                    id,
                    worker: worker.clone(),
                    process_blockers,
                    live: AtomicUsize::new(0),
                    blockers: AtomicUsize::new(1),
                }),
                worker,
                scheduled: AtomicBool::new(false),
            }),
            actor: Mutex::new(ComponentActor {
                updates: receiver,
                signals: HashMap::new(),
                dirty: false,
            }),
            snapshot: ArcSwap::from_pointee(Snapshot::default()),
            changes,
        })
    }

    pub(crate) fn report(&self, filter: Filter) -> Report {
        Report::new(Arc::clone(&self.name), self.snapshot.load_full(), filter)
    }

    pub(crate) fn is_registered(&self) -> bool {
        self.dispatch.registration.load().is_some()
    }

    pub(crate) fn process(&self, aggregate: &mut ProcessAggregate) {
        // Release: a publisher can schedule another pass while this one is running.
        self.dispatch.scheduled.store(false, Ordering::Release);
        let mut actor = self.actor.lock().unwrap_or_else(PoisonError::into_inner);
        let mut processed = 0;
        let ComponentActor { updates, signals, dirty } = &mut *actor;
        for slot in updates.try_iter().take(MAX_BATCH) {
            processed += 1;
            // AcqRel: clearing the flag before reading the slot makes a concurrent publisher
            // either observe the cleared flag and enqueue again or publish its contents before
            // this swap so the reads below observe them.
            slot.queued.swap(false, Ordering::AcqRel);
            if slot.removed.load(Ordering::Acquire) {
                *dirty |= signals.remove(&slot.id).is_some();
            } else if let Some(signal) = slot.pending.swap(None) {
                signals.insert(slot.id, signal);
                *dirty = true;
            }
        }

        if processed == MAX_BATCH {
            self.dispatch.schedule();
        }
        if !*dirty {
            drop(actor);
            aggregate.refresh(false);
            return;
        }

        let snapshot = Snapshot::collect(signals.values());
        let state = snapshot.state();
        // A recovery can coalesce into a slot ahead of another publisher's failure. Keep the
        // last whole report and retry on the next turn, even if it only removes an unseen slot.
        if state == Health::Healthy
            && self
                .dispatch
                .registration
                .load()
                .as_ref()
                .is_none_or(|r| r.blockers.load(Ordering::SeqCst) != 0)
        {
            return;
        }
        *dirty = false;
        drop(actor);
        self.snapshot.store(Arc::new(snapshot));
        aggregate.set(self.id, state);
        self.changes.send_replace(());
    }
}

/// An update path that does not own the component or its worker.
struct ComponentDispatch {
    component: Weak<ComponentShared>,
    updates: Sender<Arc<Slot>>,
    worker: Sender<Work>,
    registration: ArcSwapOption<Registration>,
    scheduled: AtomicBool,
}

/// Publishers borrow the registration only for an update, never across a notification.
struct Registration {
    id: u64,
    worker: Sender<Work>,
    process_blockers: Arc<AtomicUsize>,
    live: AtomicUsize,
    blockers: AtomicUsize,
}

impl Registration {
    fn block(&self) {
        // SeqCst orders overlapping failures and recoveries. Reserve at the root first
        // so that it is never more optimistic than the individual registrations.
        self.process_blockers.fetch_add(1, Ordering::SeqCst);
        self.blockers.fetch_add(1, Ordering::SeqCst);
    }

    fn unblock(&self) {
        self.blockers.fetch_sub(1, Ordering::SeqCst);
        self.process_blockers.fetch_sub(1, Ordering::SeqCst);
    }

    fn leave(&self, state: Health) {
        // Transfer the leaving publisher's veto to the empty component if it was the last.
        // A healthy publisher must reserve one before a concurrent registration can claim it.
        if state == Health::Healthy {
            self.block();
        }
        if self.live.fetch_sub(1, Ordering::SeqCst) != 1 {
            self.unblock();
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = self.worker.send(Work::Remove(self.id, *self.blockers.get_mut()));
    }
}

impl ComponentDispatch {
    fn enqueue(&self, slot: &Arc<Slot>) {
        // AcqRel: Release publishes the slot contents to the worker that clears this flag;
        // Acquire observes the worker's previous clear.
        if slot.queued.swap(true, Ordering::AcqRel) {
            return;
        }
        if self.updates.send(Arc::clone(slot)).is_ok() {
            self.schedule();
        }
    }

    fn schedule(&self) {
        // AcqRel: Release publishes queued updates; Acquire observes the worker's previous clear.
        if !self.scheduled.swap(true, Ordering::AcqRel) {
            // Closure means the worker stopped or its owners are gone. Startup failure is logged,
            // and an unexpected worker panic follows the application's panic policy.
            let _ = self.worker.send(Work::Component(Weak::clone(&self.component)));
        }
    }
}

/// Holds only the latest update while the publisher waits in the queue.
struct Slot {
    id: u64,
    pending: ArcSwapOption<Signal>,
    queued: AtomicBool,
    removed: AtomicBool,
}

/// One independently owned writer of a [`Component`]'s health.
///
/// A publisher stores its current [`Signal`] locally and forwards it to the component only when
/// it changes. Dropping a publisher unregisters its contribution without blocking. A publisher
/// does not keep its component alive; after the component is gone, publishing only changes the
/// local signal. Cloning creates another independent `Healthy` publisher for the same component.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Health};
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let mut startup = aggregator.component("startup");
/// let mut publisher = startup.publisher();
///
/// publisher.publish(Health::Unhealthy, [("phase", "starting")]);
/// assert_eq!(publisher.signal().state(), Health::Unhealthy);
///
/// publisher.publish(Health::Healthy, [("phase", "running")]);
/// while startup.state() != Health::Healthy {
///     startup.changed().await;
/// }
/// # });
/// ```
pub struct Publisher {
    signal: Signal,
    slot: Arc<Slot>,
    dispatch: Arc<ComponentDispatch>,
}

impl Publisher {
    fn register(component: &Arc<ComponentShared>) -> Option<Self> {
        let registration = component.dispatch.registration.load();
        let registration = registration.as_ref()?;
        let id = next_id(&component.next_publisher);
        let signal = Signal::healthy();
        let slot = Arc::new(Slot {
            id,
            pending: ArcSwapOption::from_pointee(signal.clone()),
            queued: AtomicBool::new(false),
            removed: AtomicBool::new(false),
        });
        // The first live publisher replaces the empty-component veto.
        if registration.live.fetch_add(1, Ordering::SeqCst) == 0 {
            registration.unblock();
        }
        component.dispatch.enqueue(&slot);
        Some(Self {
            signal,
            slot,
            dispatch: Arc::clone(&component.dispatch),
        })
    }

    /// Returns the current signal of this publisher.
    #[must_use]
    pub const fn signal(&self) -> &Signal {
        &self.signal
    }

    /// Replaces the signal of this publisher.
    ///
    /// The local signal is updated before this method returns and the component converges to it
    /// without waiting. Publishing a signal equal to the current one does nothing.
    pub fn publish(&mut self, state: Health, attributes: impl IntoIterator<Item = impl Into<Attribute>>) {
        self.replace(Signal::new(state, attributes.into_iter().map(Into::into).collect()));
    }

    /// Publishes `Degraded` with a single `reason` attribute.
    pub fn degraded(&mut self, reason: impl Into<AttributeString>) {
        self.replace(Signal::new(
            Health::Degraded,
            Box::new([Attribute::new(REASON, AttributeValue::String(reason.into()))]),
        ));
    }

    fn replace(&mut self, signal: Signal) {
        if signal == self.signal {
            return;
        }
        if let Some(registration) = self.dispatch.registration.load().as_ref() {
            if self.signal.state() != signal.state() {
                if signal.state() == Health::Healthy {
                    registration.unblock();
                } else if self.signal.state() == Health::Healthy {
                    registration.block();
                }
            }
            self.slot.pending.store(Some(Arc::new(signal.clone())));
            self.dispatch.enqueue(&self.slot);
        }
        self.signal = signal;
    }
}

impl Clone for Publisher {
    fn clone(&self) -> Self {
        self.dispatch
            .component
            .upgrade()
            .and_then(|component| Self::register(&component))
            .unwrap_or_else(|| Self {
                signal: Signal::healthy(),
                // Removal is permanent. Neither clone forwards to the slot again.
                slot: Arc::clone(&self.slot),
                dispatch: Arc::clone(&self.dispatch),
            })
    }
}

impl Debug for Publisher {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("signal", &self.signal)
            .finish_non_exhaustive()
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        if let Some(registration) = self.dispatch.registration.load().as_ref() {
            registration.leave(self.signal.state());
            // Release: the worker that observes removal must not apply an older pending signal.
            self.slot.removed.store(true, Ordering::Release);
            self.dispatch.enqueue(&self.slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;

    use super::*;

    #[test]
    fn empty_live_set_does_not_publish_a_ready_snapshot_between_turns() {
        let (work, pending) = mpsc::channel();
        let clients = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let mut advance = || {
            let Work::Component(component) = pending.try_recv().unwrap() else {
                panic!("expected component work");
            };
            component.upgrade().unwrap().process(&mut aggregate);
        };

        let mut first = Publisher::register(&clients).unwrap();
        first.publish(Health::Unhealthy, [("attempt", 1)]);
        let second = Publisher::register(&clients).unwrap();
        advance();
        let changes = clients.changes.subscribe();
        first.publish(Health::Unhealthy, [("attempt", 2)]);
        for _ in 1..MAX_BATCH {
            drop(Publisher::register(&clients).unwrap());
        }
        drop(second);
        drop(first);
        advance();
        let retained = clients.report(Filter::ALL);
        assert_eq!(retained.state(), Health::Unhealthy);
        assert_eq!(retained.signal_count(Health::Unhealthy), 1);
        assert_eq!(retained.signal_count(Health::Healthy), 1);
        assert!(changes.changed_async().now_or_never().is_none());
        advance();
        assert!(changes.changed_async().now_or_never().is_some());
        let empty = clients.report(Filter::ALL);
        assert_eq!(empty.state(), Health::Unhealthy);
        assert_eq!(empty.signal_count(Health::Unhealthy), 0);
        assert_eq!(empty.signal_count(Health::Healthy), 0);
    }

    #[test]
    fn overlapping_failures_do_not_publish_a_ready_snapshot_between_turns() {
        let (work, pending) = mpsc::channel();
        let clients = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let mut advance = || {
            let Work::Component(component) = pending.try_recv().unwrap() else {
                panic!("expected component work");
            };
            component.upgrade().unwrap().process(&mut aggregate);
        };

        let mut first = Publisher::register(&clients).unwrap();
        first.publish(Health::Unhealthy, [("attempt", 1)]);
        advance();
        assert_eq!(clients.report(Filter::ALL).signal_count(Health::Unhealthy), 1);

        first.publish(Health::Unhealthy, [("attempt", 2)]);
        let _healthy: Vec<_> = (1..MAX_BATCH).map(|_| Publisher::register(&clients).unwrap()).collect();
        let mut second = Publisher::register(&clients).unwrap();
        second.degraded("retrying");
        second.publish(Health::Unhealthy, [("attempt", 1)]);
        first.publish(Health::Healthy, [("attempt", 3)]);
        advance();
        assert_eq!(clients.report(Filter::ALL).state(), Health::Unhealthy);

        // Removing the second publisher before its first snapshot is otherwise a no-op.
        drop(second);
        advance();
        let recovered = clients.report(Filter::ALL);
        assert_eq!(recovered.state(), Health::Healthy);
        assert_eq!(recovered.signal_count(Health::Healthy), MAX_BATCH);
    }

    #[test]
    fn queued_bursts_coalesce_and_resume_without_another_publish() {
        let (work, pending) = mpsc::channel();
        let clients = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let changes = clients.changes.subscribe();
        let mut publishers: Vec<_> = (0..=MAX_BATCH).map(|_| Publisher::register(&clients).unwrap()).collect();
        for publisher in &mut publishers {
            publisher.degraded("busy");
        }
        let advance = |aggregate: &mut ProcessAggregate| {
            let Work::Component(component) = pending.try_recv().unwrap() else {
                panic!("expected component work");
            };
            assert!(pending.try_recv().is_err(), "a pending component needs only one wake");
            component.upgrade().unwrap().process(aggregate);
            assert!(changes.changed_async().now_or_never().is_some());
        };
        advance(&mut aggregate);
        assert_eq!(clients.report(Filter::ALL).signal_count(Health::Degraded), MAX_BATCH);
        advance(&mut aggregate);
        assert_eq!(clients.report(Filter::ALL).signal_count(Health::Degraded), publishers.len());
        assert!(pending.try_recv().is_err(), "idle components must not spin");

        publishers[0].degraded("busy");
        assert!(pending.try_recv().is_err(), "equal publications must not wake the worker");
        drop(publishers.pop());
        advance(&mut aggregate);
        assert_eq!(clients.report(Filter::ALL).signal_count(Health::Degraded), publishers.len());
    }

    #[test]
    fn retired_publishers_operate_locally_even_while_a_pass_owns_the_component() {
        let (work, pending) = mpsc::channel();
        let clients = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let worker = Worker::new(ProcessAggregate::new(Weak::new()));
        worker.send(Work::Register(1));
        let handle = Component::new(ComponentOwner::new(Arc::clone(&clients), worker));
        let mut publisher = handle.publisher();
        publisher.degraded("retiring");
        assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
        clients.process(&mut ProcessAggregate::new(Weak::new()));
        let admitted = clients.dispatch.registration.load_full().unwrap();
        drop(handle);
        let mut orphan = publisher.clone();
        assert_eq!(orphan.signal().state(), Health::Healthy);
        assert_eq!(publisher.signal().state(), Health::Degraded);
        orphan.degraded("local");
        assert_eq!(orphan.signal().state(), Health::Degraded);
        drop((orphan, publisher));
        assert!(
            pending.try_recv().is_err(),
            "orphaned registration, publication and cleanup must leave the worker idle"
        );
        drop(admitted);
        assert!(
            matches!(pending.try_recv(), Ok(Work::Remove(1, 2))),
            "retirement releases the old failure and removal veto together"
        );
    }

    #[test]
    fn publisher_removed_before_first_snapshot_does_not_notify() {
        let (work, pending) = mpsc::channel();
        let clients = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let mut advance = || {
            assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
            clients.process(&mut aggregate);
        };
        let changes = clients.changes.subscribe();
        let mut retained = Vec::new();
        while retained.len() < 2 {
            drop(Publisher::register(&clients).unwrap());
            advance();
            assert!(changes.changed_async().now_or_never().is_none());
            assert_eq!(clients.report(Filter::ALL).signal_count(Health::Healthy), retained.len());

            retained.push(Publisher::register(&clients).unwrap());
            advance();
            assert!(changes.changed_async().now_or_never().is_some());
            assert_eq!(clients.report(Filter::ALL).signal_count(Health::Healthy), retained.len());
        }
    }

    #[test]
    fn enqueue_racing_receiver_teardown_does_not_wake_the_worker() {
        let (work, pending) = mpsc::channel();
        let clients = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let publisher = Publisher::register(&clients).unwrap();
        assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
        clients.process(&mut ProcessAggregate::new(Weak::new()));

        // The publisher has checked registration, but the receiver disappears before enqueue.
        drop(clients);
        publisher.dispatch.enqueue(&publisher.slot);
        assert!(pending.try_recv().is_err(), "there is no component left to process");
    }
}
