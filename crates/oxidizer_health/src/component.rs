// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Weak};

use arc_swap::{ArcSwap, ArcSwapOption};
use foldhash::HashMap;
use performables::sync::channel::{WatchSender, watch};
use thread_aware::{Thread, ThreadAware};

use crate::aggregator::{ProcessAggregate, next_id};
use crate::report::Snapshot;
use crate::worker::{Changes, Work, Worker};
use crate::{Attribute, Health, Report, Signal};

// Bound a busy component's turn without rebuilding its snapshot after every slot.
const MAX_BATCH: usize = 256;

/// One logical application subsystem, aggregating the signals of its publishers.
///
/// A component's health is the worst signal among its active publishers. A component without
/// publishers is `Unhealthy`. Clones refer to the same logical component; each clone has an
/// independent [`changed`](Self::changed) notification cursor. The component stops and
/// unregisters from its [`Aggregator`](crate::Aggregator) after its last handle and publisher drop.
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
    changes: Changes,
}

impl Component {
    pub(crate) fn new(owner: Arc<ComponentOwner>) -> Self {
        let changes = owner.worker.subscribe(&owner.shared.changes);
        Self { owner, changes }
    }

    /// Creates an independently owned publisher in `Healthy` state with no attributes.
    ///
    /// This is shorthand for [`publisher_with`](Self::publisher_with) with a healthy signal.
    #[must_use]
    pub fn publisher(&self) -> Publisher {
        Publisher::register_owned(&self.owner, Signal::healthy())
    }

    /// Creates an independently owned publisher with its initial signal.
    ///
    /// The publisher stores the signal before its first aggregation turn. If the state is not
    /// `Healthy`, it also blocks readiness before that turn. Component and process aggregation
    /// remain asynchronous after admission.
    ///
    /// # Examples
    ///
    /// ```
    /// # testing_aids::init_tracing!();
    /// use oxidizer_health::{Aggregator, Health};
    ///
    /// let aggregator = Aggregator::new();
    /// let database = aggregator.component("database");
    /// let publisher = database.publisher_with(Health::Degraded, [("reason", "warming")]);
    ///
    /// assert_eq!(publisher.signal().state(), Health::Degraded);
    /// ```
    #[must_use]
    pub fn publisher_with(&self, state: Health, attributes: impl IntoIterator<Item = impl Into<Attribute>>) -> Publisher {
        Publisher::register_owned(&self.owner, Signal::new(state, attributes.into_iter().map(Into::into).collect()))
    }

    /// Waits until the component state or report data may have changed.
    ///
    /// Several updates can produce one notification. Read the latest [`state`](Self::state)
    /// or [`report`](Self::report) rather than expecting a notification for each transition.
    /// The worker-exit rules are the same as for [`Monitor::changed`](crate::Monitor::changed).
    pub async fn changed(&mut self) {
        self.changes.wait(&self.owner.worker).await;
    }

    /// Returns the current aggregate component state.
    ///
    /// Returns [`Health::Unhealthy`] if the worker has stopped.
    #[must_use]
    pub fn state(&self) -> Health {
        if self.owner.worker.is_running() {
            self.owner.shared.snapshot.load().state()
        } else {
            Health::Unhealthy
        }
    }

    /// Returns a consistent snapshot of the component's publisher signals.
    ///
    /// If the worker has stopped, this returns its last snapshot. The report can still show
    /// `Healthy` even though [`state`](Self::state) returns `Unhealthy`.
    #[expect(
        clippy::unused_async,
        reason = "the contract allows report assembly to wait for the component owner"
    )]
    pub async fn report(&self) -> Report {
        self.owner.shared.report()
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

/// The last published snapshot outlives the worker's mutable state and slot queue.
pub(crate) struct ComponentShared {
    name: Arc<str>,
    pub(crate) id: u64,
    next_publisher: AtomicU64,
    dispatch: Arc<ComponentDispatch>,
    snapshot: ArcSwap<Snapshot>,
    changes: WatchSender<()>,
}

/// Only the worker owns the receiver, so its exit releases queued slots even if readers survive.
pub(crate) struct ComponentActor {
    updates: Receiver<Arc<Slot>>,
    signals: HashMap<u64, Arc<Signal>>,
    dirty: bool,
}

impl ComponentShared {
    pub(crate) fn new(name: Arc<str>, id: u64, worker: Sender<Work>, process_blockers: Arc<AtomicUsize>) -> (Arc<Self>, ComponentActor) {
        let (updates, receiver) = mpsc::channel();
        let (changes, _) = watch(());
        // Register the empty-component veto before any publisher can obtain the component.
        process_blockers.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::new_cyclic(|component| Self {
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
            snapshot: ArcSwap::from_pointee(Snapshot::default()),
            changes,
        });
        let actor = ComponentActor {
            updates: receiver,
            signals: HashMap::default(),
            dirty: false,
        };
        (shared, actor)
    }

    pub(crate) fn report(&self) -> Report {
        Report::new(Arc::clone(&self.name), self.snapshot.load_full())
    }

    pub(crate) fn is_registered(&self) -> bool {
        self.dispatch.registration.load().is_some()
    }

    pub(crate) fn process(&self, actor: &mut ComponentActor, aggregate: &mut ProcessAggregate) {
        // Release: a publisher can schedule another pass while this one is running.
        self.dispatch.scheduled.store(false, Ordering::Release);
        let mut processed = 0;
        let ComponentActor { updates, signals, dirty } = actor;
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
///
/// An empty component or a non-healthy publisher blocks readiness. Reserve at the process
/// before the component and release in reverse order: between operations the process must not
/// be more optimistic. The first healthy publisher releases the empty component's reservation;
/// a departing last publisher leaves one behind. All these transitions and the worker's zero
/// checks use the same `SeqCst` order, but the counter updates are not a single transaction.
struct Registration {
    id: u64,
    worker: Sender<Work>,
    process_blockers: Arc<AtomicUsize>,
    live: AtomicUsize,
    blockers: AtomicUsize,
}

impl Registration {
    fn block(&self) {
        self.process_blockers.fetch_add(1, Ordering::SeqCst);
        self.blockers.fetch_add(1, Ordering::SeqCst);
    }

    fn unblock(&self) {
        let component_before = self.blockers.fetch_sub(1, Ordering::SeqCst);
        let process_before = self.process_blockers.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(
            component_before > 0 && process_before > 0,
            "a release needs an existing reservation"
        );
    }

    fn admit(&self, state: Health) {
        if state != Health::Healthy {
            self.block();
        }
        if self.live.fetch_add(1, Ordering::SeqCst) == 0 {
            self.unblock();
        }
    }

    fn change_health(&self, previous: Health, next: Health) {
        match (previous == Health::Healthy, next == Health::Healthy) {
            (true, false) => self.block(),
            (false, true) => self.unblock(),
            _ => {}
        }
    }

    fn leave(&self, state: Health) {
        // Transfer the leaving publisher's veto to the empty component if it was the last.
        // A healthy publisher must reserve one before a concurrent registration can claim it.
        if state == Health::Healthy {
            self.block();
        }
        let live_before = self.live.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(live_before > 0, "only a live publisher can leave");
        if live_before != 1 {
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
/// keeps its component registered until the publisher and all component handles are dropped.
///
/// A clone represents a new publisher, not another handle to the original signal. It starts
/// `Healthy` with no attributes, even if the original is `Degraded`. If only the healthy clone
/// remains after the original drops, the component can become `Healthy`.
///
/// For one publisher per worker thread, pass a [`Component`] to each worker and create its
/// publisher there. The component implements [`ThreadAware`]; the publisher does not.
///
/// # Examples
///
/// To start with a signal that is not `Healthy`, use [`Component::publisher_with`]. It admits the
/// signal and its readiness reservation together. Aggregation still runs in the background.
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Health};
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let monitor = aggregator.monitor();
/// let mut startup = aggregator.component("startup");
/// assert_eq!(monitor.state(), Health::Unhealthy);
///
/// let mut publisher = startup.publisher_with(Health::Unhealthy, [("phase", "starting")]);
/// assert_eq!(publisher.signal().state(), Health::Unhealthy);
///
/// // Initialization succeeded. Keep this publisher to report later changes.
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
    owner: Option<Arc<ComponentOwner>>,
}

impl Publisher {
    fn register(component: &Arc<ComponentShared>) -> Option<Self> {
        Self::register_inner(component, None, Signal::healthy())
    }

    fn register_owned(owner: &Arc<ComponentOwner>, signal: Signal) -> Self {
        Self::register_inner(&owner.shared, Some(Arc::clone(owner)), signal).expect("the component handle keeps its registration alive")
    }

    fn register_inner(component: &Arc<ComponentShared>, owner: Option<Arc<ComponentOwner>>, signal: Signal) -> Option<Self> {
        let registration = component.dispatch.registration.load();
        let registration = registration.as_ref()?;
        let id = next_id(&component.next_publisher);
        let slot = Arc::new(Slot {
            id,
            pending: ArcSwapOption::from_pointee(signal.clone()),
            queued: AtomicBool::new(false),
            removed: AtomicBool::new(false),
        });
        registration.admit(signal.state());
        component.dispatch.enqueue(&slot);
        Some(Self {
            signal,
            slot,
            dispatch: Arc::clone(&component.dispatch),
            owner,
        })
    }

    /// Returns the current signal of this publisher.
    #[must_use]
    pub const fn signal(&self) -> &Signal {
        &self.signal
    }

    /// Replaces the signal of this publisher.
    ///
    /// [`Self::signal`] reflects the update before this method returns. Aggregation runs in the
    /// background and can skip intermediate signals. Publishing a signal equal to the current one
    /// does nothing.
    pub fn publish(&mut self, state: Health, attributes: impl IntoIterator<Item = impl Into<Attribute>>) {
        self.replace(Signal::new(state, attributes.into_iter().map(Into::into).collect()));
    }

    /// Publishes `Degraded` with a single `reason` attribute.
    pub fn degraded(&mut self, reason: impl Into<Arc<str>>) {
        let reason: Arc<str> = reason.into();
        self.replace(Signal::new(Health::Degraded, Box::new([Attribute::from(("reason", reason))])));
    }

    fn replace(&mut self, signal: Signal) {
        if signal == self.signal {
            return;
        }
        if let Some(registration) = self.dispatch.registration.load().as_ref() {
            registration.change_health(self.signal.state(), signal.state());
            self.slot.pending.store(Some(Arc::new(signal.clone())));
            self.dispatch.enqueue(&self.slot);
        }
        self.signal = signal;
    }
}

impl Clone for Publisher {
    fn clone(&self) -> Self {
        if let Some(owner) = &self.owner {
            return Self::register_owned(owner, Signal::healthy());
        }
        self.dispatch
            .component
            .upgrade()
            .and_then(|component| Self::register(&component))
            .unwrap_or_else(|| Self {
                signal: Signal::healthy(),
                // Removal is permanent. Neither clone forwards to the slot again.
                slot: Arc::clone(&self.slot),
                dispatch: Arc::clone(&self.dispatch),
                owner: None,
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
    fn component_shared_retains_its_registration_id() {
        let (work, _pending) = mpsc::channel();
        let (shared, _actor) = ComponentShared::new(Arc::from("clients"), 7, work, Arc::default());
        let registration = shared.dispatch.registration.load();

        assert_eq!(shared.id, 7);
        assert_eq!(registration.as_ref().unwrap().id, 7);
    }

    #[test]
    fn component_publishers_retain_the_component_owner() {
        use crate::Aggregator;

        let root = Aggregator::new();
        let component = root.component("clients");
        let publisher = component.publisher();

        assert!(publisher.owner.is_some());
    }

    #[test]
    fn aggregator_clones_share_component_owners() {
        use crate::Aggregator;

        let root = Aggregator::new();
        let clone = root.clone();
        let first = root.component("clients");
        let second = clone.component("clients");

        assert!(Arc::ptr_eq(&first.owner, &second.owner));
    }

    #[test]
    fn publisher_registration_initializes_and_enqueues_its_slot() {
        let (work, pending) = mpsc::channel();
        let (clients, _actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let publisher = Publisher::register(&clients).unwrap();

        assert!(publisher.slot.queued.load(Ordering::Acquire));
        assert!(!publisher.slot.removed.load(Ordering::Acquire));
        assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
    }

    #[test]
    fn initial_nonhealthy_publisher_keeps_readiness_reserved_before_its_turn() {
        let process_blockers = Arc::new(AtomicUsize::new(0));
        let (work, pending) = mpsc::channel();
        let (clients, _actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::clone(&process_blockers));

        let publisher = Publisher::register_inner(
            &clients,
            None,
            Signal::new(Health::Unhealthy, Box::new([Attribute::from(("reason", "starting"))])),
        )
        .unwrap();

        assert_eq!(publisher.signal().state(), Health::Unhealthy);
        assert_eq!(process_blockers.load(Ordering::SeqCst), 1);
        assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
    }

    #[test]
    #[cfg_attr(panic = "abort", ignore = "requires worker unwinding")]
    fn worker_exit_releases_slots_queued_before_and_after_failure() {
        use std::cell::OnceCell;
        use std::pin::pin;
        use std::task::{Context, Wake, Waker};
        use std::time::Duration;

        use crate::Aggregator;

        fn receive<T>(rx: &Receiver<T>) -> Result<T, mpsc::RecvTimeoutError> {
            if testing_aids::is_mutation_testing() {
                rx.recv().map_err(Into::into)
            } else {
                rx.recv_timeout(Duration::from_secs(5))
            }
        }

        fn transient_slot(component: &Component) -> Weak<Slot> {
            let mut publisher = component.publisher();
            publisher.degraded("local signal");
            assert_eq!(publisher.signal().state(), Health::Degraded);
            Arc::downgrade(&publisher.slot)
        }

        struct Fail(Component, Sender<(Weak<Slot>, Receiver<()>)>);
        impl Wake for Fail {
            fn wake(self: Arc<Self>) {
                thread_local! { static ALIVE: OnceCell<Sender<()>> = const { OnceCell::new() }; }
                ALIVE.with(|alive| {
                    alive.get_or_init(|| {
                        let (sender, exited) = mpsc::channel();
                        self.1.send((transient_slot(&self.0), exited)).unwrap();
                        sender
                    });
                });
                panic!("simulated observer failure");
            }
        }

        let root = Aggregator::new();
        let mut component = root.component("database");
        let mut writer = component.publisher();
        futures::executor::block_on(component.changed());
        drop(root);
        let (queued, received) = mpsc::channel();
        let waker = Waker::from(Arc::new(Fail(component.clone(), queued)));
        let mut cursor = component.clone();
        let mut changed = pin!(cursor.changed());
        assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        writer.degraded("trigger");
        let (before, exited) = receive(&received).unwrap();
        assert_eq!(receive(&exited), Err(mpsc::RecvTimeoutError::Disconnected));
        let after = transient_slot(&component);
        assert_eq!([before.strong_count(), after.strong_count()], [0, 0]);
    }

    #[test]
    fn empty_live_set_does_not_publish_a_ready_snapshot_between_turns() {
        let (work, pending) = mpsc::channel();
        let (clients, mut actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let mut advance = || {
            let Work::Component(component) = pending.try_recv().unwrap() else {
                panic!("expected component work");
            };
            component.upgrade().unwrap().process(&mut actor, &mut aggregate);
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
        let retained = clients.report();
        assert_eq!(retained.state(), Health::Unhealthy);
        assert_eq!(retained.publisher_count(Health::Unhealthy), 1);
        assert_eq!(retained.publisher_count(Health::Healthy), 1);
        assert!(changes.changed_async().now_or_never().is_none());
        advance();
        assert!(changes.changed_async().now_or_never().is_some());
        let empty = clients.report();
        assert_eq!(empty.state(), Health::Unhealthy);
        assert_eq!(empty.publisher_count(Health::Unhealthy), 0);
        assert_eq!(empty.publisher_count(Health::Healthy), 0);
    }

    #[test]
    fn overlapping_failures_do_not_publish_a_ready_snapshot_between_turns() {
        let (work, pending) = mpsc::channel();
        let (clients, mut actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let mut advance = || {
            let Work::Component(component) = pending.try_recv().unwrap() else {
                panic!("expected component work");
            };
            component.upgrade().unwrap().process(&mut actor, &mut aggregate);
        };

        let mut first = Publisher::register(&clients).unwrap();
        first.publish(Health::Unhealthy, [("attempt", 1)]);
        advance();
        assert_eq!(clients.report().publisher_count(Health::Unhealthy), 1);

        first.publish(Health::Unhealthy, [("attempt", 2)]);
        let _healthy: Vec<_> = (1..MAX_BATCH).map(|_| Publisher::register(&clients).unwrap()).collect();
        let mut second = Publisher::register(&clients).unwrap();
        second.degraded("retrying");
        second.publish(Health::Unhealthy, [("attempt", 1)]);
        first.publish(Health::Healthy, [("attempt", 3)]);
        advance();
        assert_eq!(clients.report().state(), Health::Unhealthy);

        // Removing the second publisher before its first snapshot is otherwise a no-op.
        drop(second);
        advance();
        let recovered = clients.report();
        assert_eq!(recovered.state(), Health::Healthy);
        assert_eq!(recovered.publisher_count(Health::Healthy), MAX_BATCH);
    }

    #[test]
    fn queued_bursts_coalesce_and_resume_without_another_publish() {
        let (work, pending) = mpsc::channel();
        let (clients, mut actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let changes = clients.changes.subscribe();
        let mut publishers: Vec<_> = (0..=MAX_BATCH).map(|_| Publisher::register(&clients).unwrap()).collect();
        for publisher in &mut publishers {
            publisher.degraded("busy");
        }
        let mut advance = |aggregate: &mut ProcessAggregate| {
            let Work::Component(component) = pending.try_recv().unwrap() else {
                panic!("expected component work");
            };
            assert!(pending.try_recv().is_err(), "a pending component needs only one wake");
            component.upgrade().unwrap().process(&mut actor, aggregate);
            assert!(changes.changed_async().now_or_never().is_some());
        };
        advance(&mut aggregate);
        assert_eq!(clients.report().publisher_count(Health::Degraded), MAX_BATCH);
        advance(&mut aggregate);
        assert_eq!(clients.report().publisher_count(Health::Degraded), publishers.len());
        assert!(pending.try_recv().is_err(), "idle components must not spin");

        publishers[0].degraded("busy");
        assert!(pending.try_recv().is_err(), "equal publications must not wake the worker");
        drop(publishers.pop());
        advance(&mut aggregate);
        assert_eq!(clients.report().publisher_count(Health::Degraded), publishers.len());
    }

    #[test]
    fn retired_publishers_operate_locally_even_while_a_pass_owns_the_component() {
        let (work, pending) = mpsc::channel();
        let (clients, mut actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let worker = Worker::new(ProcessAggregate::new(Weak::new()));
        let handle = Component::new(ComponentOwner::new(Arc::clone(&clients), worker));
        let mut publisher = Publisher::register(&clients).unwrap();
        publisher.degraded("retiring");
        assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
        clients.process(&mut actor, &mut ProcessAggregate::new(Weak::new()));
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
        let (clients, mut actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let mut aggregate = ProcessAggregate::new(Weak::new());
        let mut advance = || {
            assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
            clients.process(&mut actor, &mut aggregate);
        };
        let changes = clients.changes.subscribe();
        let mut retained = Vec::new();
        while retained.len() < 2 {
            drop(Publisher::register(&clients).unwrap());
            advance();
            assert!(changes.changed_async().now_or_never().is_none());
            assert_eq!(clients.report().publisher_count(Health::Healthy), retained.len());

            retained.push(Publisher::register(&clients).unwrap());
            advance();
            assert!(changes.changed_async().now_or_never().is_some());
            assert_eq!(clients.report().publisher_count(Health::Healthy), retained.len());
        }
    }

    #[test]
    fn retiring_a_component_discards_in_flight_slots_while_the_worker_stays_live() {
        use futures::executor::block_on;

        use crate::Aggregator;

        let root = Aggregator::new();
        let mut component = root.component("retiring");
        let publisher = Publisher::register(&component.owner.shared).unwrap();
        block_on(component.changed());
        drop(component);

        // This independent publication follows registration removal on the same worker.
        let mut probe = root.component("probe");
        let _live = probe.publisher();
        block_on(probe.changed());
        // A publisher already admitted before retirement can still reach enqueue.
        publisher.dispatch.enqueue(&publisher.slot);
        let retained = Arc::downgrade(&publisher.slot);
        drop(publisher);
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn enqueue_racing_receiver_teardown_does_not_wake_the_worker() {
        let (work, pending) = mpsc::channel();
        let (clients, mut actor) = ComponentShared::new(Arc::from("clients"), 1, work, Arc::default());
        let publisher = Publisher::register(&clients).unwrap();
        assert!(matches!(pending.try_recv(), Ok(Work::Component(_))));
        clients.process(&mut actor, &mut ProcessAggregate::new(Weak::new()));

        // The publisher has checked registration, but the receiver disappears before enqueue.
        drop(clients);
        drop(actor);
        publisher.dispatch.enqueue(&publisher.slot);
        assert!(pending.try_recv().is_err(), "there is no component left to process");
    }
}
