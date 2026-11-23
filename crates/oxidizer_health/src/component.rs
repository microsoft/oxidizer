// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use arc_swap::{ArcSwap, ArcSwapOption};
use performables::sync::channel::{WatchReceiver, WatchSender, watch};
use thread_aware::{Thread, ThreadAware};

use crate::aggregator::{AggregatorShared, ComponentCell, wait};
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
    pub fn publisher(&self) -> Publisher {
        Publisher::register(&self.owner.shared)
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
            self.owner.shared.cell.state()
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
        self.shared.cell.mark_removed();
        if let Some(aggregator) = self.shared.aggregator.upgrade() {
            aggregator.component_removed(self.shared.cell.id());
        }
    }
}

/// Owner of one component's aggregation state.
///
/// Publishers hand their latest signal to a per-publisher [`Slot`] and enqueue the slot at most
/// once while it is pending. The shared worker applies queued slots to the actor state and
/// publishes snapshots.
pub(crate) struct ComponentShared {
    name: Arc<str>,
    cell: Arc<ComponentCell>,
    aggregator: Weak<AggregatorShared>,
    next_publisher: AtomicU64,
    dispatch: Arc<ComponentDispatch>,
    actor: Mutex<ComponentActor>,
    snapshot: ArcSwap<Snapshot>,
    changes: WatchSender<()>,
}

struct ComponentActor {
    updates: Receiver<Arc<Slot>>,
    signals: HashMap<u64, Arc<Signal>>,
}

impl ComponentShared {
    pub(crate) fn new(name: Arc<str>, cell: Arc<ComponentCell>, aggregator: Weak<AggregatorShared>, worker: &Worker) -> Arc<Self> {
        let (updates, receiver) = mpsc::channel();
        let (changes, _) = watch(());
        let sender = worker.sender();
        Arc::new_cyclic(|component| Self {
            name,
            cell,
            aggregator,
            next_publisher: AtomicU64::new(0),
            dispatch: Arc::new(ComponentDispatch {
                component: Weak::clone(component),
                updates,
                worker: sender,
                scheduled: AtomicBool::new(false),
            }),
            actor: Mutex::new(ComponentActor {
                updates: receiver,
                signals: HashMap::new(),
            }),
            snapshot: ArcSwap::from_pointee(Snapshot::default()),
            changes,
        })
    }

    pub(crate) fn report(&self, filter: Filter) -> Report {
        Report::new(Arc::clone(&self.name), self.snapshot.load_full(), filter)
    }

    pub(crate) fn is_registered(&self) -> bool {
        self.cell.is_registered()
    }

    pub(crate) fn process(&self) {
        // Release: a publisher can schedule another pass while this one is running.
        self.dispatch.scheduled.store(false, Ordering::Release);
        if !self.is_registered() {
            return;
        }

        let mut actor = self.actor.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        let mut processed = 0;
        while processed < MAX_BATCH {
            let Ok(slot) = actor.updates.try_recv() else {
                break;
            };
            processed += 1;
            // AcqRel: clearing the flag before reading the slot makes a concurrent publisher
            // either observe the cleared flag and enqueue again or publish its contents before
            // this swap so the reads below observe them.
            slot.queued.swap(false, Ordering::AcqRel);
            if slot.removed.load(Ordering::Acquire) {
                changed |= actor.signals.remove(&slot.id).is_some();
            } else if let Some(signal) = slot.pending.swap(None) {
                actor.signals.insert(slot.id, signal);
                changed = true;
            }
        }

        if processed == MAX_BATCH {
            self.dispatch.schedule();
        }
        if !changed {
            return;
        }

        let snapshot = Snapshot::collect(actor.signals.values());
        drop(actor);
        let state = snapshot.state();
        self.snapshot.store(Arc::new(snapshot));
        self.cell.set_state(state);
        if let Some(aggregator) = self.aggregator.upgrade() {
            aggregator.component_changed(&self.cell);
        }
        self.changes.send_replace(());
    }
}

/// An update path that does not own the component or its worker.
struct ComponentDispatch {
    component: Weak<ComponentShared>,
    updates: Sender<Arc<Slot>>,
    worker: Sender<Work>,
    scheduled: AtomicBool,
}

impl ComponentDispatch {
    fn enqueue(&self, slot: &Arc<Slot>) {
        // AcqRel: Release publishes the slot contents to the worker that clears this flag;
        // Acquire observes the worker's previous clear.
        if !slot.queued.swap(true, Ordering::AcqRel) && self.updates.send(Arc::clone(slot)).is_err() {
            // The component is gone, so an orphaned publisher only updates its local signal.
            return;
        }
        self.schedule();
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
    fn register(component: &Arc<ComponentShared>) -> Self {
        // Relaxed: the identifier only needs to be unique.
        let id = component.next_publisher.fetch_add(1, Ordering::Relaxed);
        let signal = Signal::healthy();
        let slot = Arc::new(Slot {
            id,
            pending: ArcSwapOption::from_pointee(signal.clone()),
            queued: AtomicBool::new(false),
            removed: AtomicBool::new(false),
        });
        component.dispatch.enqueue(&slot);
        Self {
            signal,
            slot,
            dispatch: Arc::clone(&component.dispatch),
        }
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
        self.slot.pending.store(Some(Arc::new(signal.clone())));
        self.dispatch.enqueue(&self.slot);
        self.signal = signal;
    }
}

impl Clone for Publisher {
    fn clone(&self) -> Self {
        match self.dispatch.component.upgrade() {
            Some(component) if component.is_registered() => Self::register(&component),
            _ => Self {
                signal: Signal::healthy(),
                slot: Arc::new(Slot {
                    id: 0,
                    pending: ArcSwapOption::empty(),
                    queued: AtomicBool::new(false),
                    removed: AtomicBool::new(false),
                }),
                dispatch: Arc::clone(&self.dispatch),
            },
        }
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
        // Release: the drainer that observes this flag must not apply an older pending signal.
        self.slot.removed.store(true, Ordering::Release);
        self.dispatch.enqueue(&self.slot);
    }
}
