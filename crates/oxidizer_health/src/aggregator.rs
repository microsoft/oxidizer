// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use performables::sync::channel::{WatchSender, watch};
use thread_aware::{Thread, ThreadAware};

use crate::component::{ComponentOwner, ComponentShared};
use crate::worker::{Changes, Work, Worker};
use crate::{Component, Filter, Health, Reports};

/// The process-level health root.
///
/// The process reports the worst state among its registered components, or `Unhealthy` if it has none.
/// Clones share that state but track [`changed`](Self::changed) notifications independently.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Filter, Health};
///
/// # futures::executor::block_on(async {
/// let mut aggregator = Aggregator::new();
/// assert_eq!(aggregator.state(), Health::Unhealthy);
///
/// let database = aggregator.component("database");
/// let cache = aggregator.component("cache");
/// let _database_publisher = database.publisher();
/// let mut cache_publisher = cache.publisher();
/// while aggregator.state() != Health::Healthy {
///     aggregator.changed().await;
/// }
///
/// cache_publisher.degraded("evicting");
/// while aggregator.state() != Health::Degraded {
///     aggregator.changed().await;
/// }
///
/// let unhealthy = aggregator
///     .reports(Filter::DEGRADED | Filter::UNHEALTHY)
///     .await;
/// assert_eq!(unhealthy.len(), 2);
/// # });
/// ```
pub struct Aggregator {
    shared: Arc<AggregatorShared>,
    changes: Changes,
}

impl Aggregator {
    /// Starts in `Unhealthy` state with no registered components.
    #[must_use]
    pub fn new() -> Self {
        let shared = AggregatorShared::new();
        let changes = shared.worker.subscribe(&shared.changes);
        Self { shared, changes }
    }

    /// Returns the component registered under `name`, registering it on first use.
    ///
    /// # Panics
    ///
    /// Panics if `name` is empty.
    #[must_use]
    pub fn component(&self, name: impl AsRef<str>) -> Component {
        let name = name.as_ref();
        assert!(!name.is_empty(), "component name must not be empty");
        self.shared.component(name)
    }

    /// Subscribes to process health without granting access to register components.
    #[must_use]
    pub fn monitor(&self) -> Monitor {
        Monitor {
            shared: Arc::clone(&self.shared),
            changes: self.shared.worker.subscribe(&self.shared.changes),
        }
    }

    /// Waits until the aggregate state or report data may have changed.
    ///
    /// Intermediate transitions can be coalesced. Read [`state`](Self::state) or
    /// [`reports`](Self::reports) afterwards to observe the current state.
    pub async fn changed(&mut self) {
        self.changes.wait(&self.shared.worker).await;
    }

    /// Returns the latest process state. It can briefly lag component registration, removal, or publication.
    #[must_use]
    pub fn state(&self) -> Health {
        self.shared.state()
    }

    /// Returns consistent snapshots of the registered components.
    ///
    /// Iteration order is unspecified.
    #[expect(
        clippy::unused_async,
        reason = "the contract allows report assembly to wait for the component owners"
    )]
    pub async fn reports(&self, filter: Filter) -> Reports {
        self.shared.reports(filter)
    }
}

impl Default for Aggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Aggregator {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            changes: self.changes.clone(),
        }
    }
}

impl Debug for Aggregator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

// Relocation keeps the handle to the same process root; the shared owner is thread-safe.
impl ThreadAware for Aggregator {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

/// A read-only handle to the process health of an [`Aggregator`].
///
/// Each clone has an independent [`changed`](Self::changed) notification cursor.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Health};
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let database = aggregator.component("database");
/// let mut monitor = aggregator.monitor();
/// let _publisher = database.publisher();
/// while monitor.state() != Health::Healthy {
///     monitor.changed().await;
/// }
/// # });
/// ```
pub struct Monitor {
    shared: Arc<AggregatorShared>,
    changes: Changes,
}

impl Monitor {
    /// Waits until the aggregate state or report data may have changed.
    ///
    /// Intermediate transitions can be coalesced. Read [`state`](Self::state) or
    /// [`reports`](Self::reports) afterwards to observe the current state.
    pub async fn changed(&mut self) {
        self.changes.wait(&self.shared.worker).await;
    }

    /// Returns the latest process state, with the same update timing as [`Aggregator::state`].
    #[must_use]
    pub fn state(&self) -> Health {
        self.shared.state()
    }

    /// Returns consistent snapshots of the registered components.
    ///
    /// Iteration order is unspecified.
    #[expect(
        clippy::unused_async,
        reason = "the contract allows report assembly to wait for the component owners"
    )]
    pub async fn reports(&self, filter: Filter) -> Reports {
        self.shared.reports(filter)
    }
}

impl Clone for Monitor {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            changes: self.changes.clone(),
        }
    }
}

impl Debug for Monitor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

// Relocation keeps the handle to the same process root; the shared owner is thread-safe.
impl ThreadAware for Monitor {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[expect(clippy::cast_possible_truncation, reason = "health indices are below Health::COUNT")]
const fn health_bits(state: Health) -> u8 {
    state.index() as u8
}

pub(crate) struct AggregatorShared {
    next_component: AtomicU64,
    registry: Mutex<HashMap<Arc<str>, RegistryEntry>>,
    worker: Arc<Worker>,
    blockers: Arc<AtomicUsize>,
    health: AtomicU8,
    changes: WatchSender<()>,
}

pub(crate) fn next_id(counter: &AtomicU64) -> u64 {
    // Relaxed: IDs ensure uniqueness only. Reusing an ID could remove a different registration.
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("health registration IDs exhausted")
}

struct RegistryEntry {
    owner: Weak<ComponentOwner>,
    shared: Weak<ComponentShared>,
}

/// Owned only by the worker. A component ID is never reused, even if its name is registered again.
pub(crate) struct ProcessAggregate {
    root: Weak<AggregatorShared>,
    components: HashMap<u64, Health>,
    counts: [usize; Health::COUNT],
}

impl AggregatorShared {
    fn new() -> Arc<Self> {
        let (changes, _) = watch(());
        Arc::new_cyclic(|root| Self {
            next_component: AtomicU64::default(),
            registry: Mutex::new(HashMap::new()),
            worker: Worker::new(ProcessAggregate::new(Weak::clone(root))),
            blockers: Arc::default(),
            // #[gamma::skip(expr.increment, tag = "equivalent", reason = "Both 2 and 3 decode to Unhealthy; the raw byte is private and subsequent writes replace it.")]
            health: AtomicU8::new(health_bits(Health::Unhealthy)),
            changes,
        })
    }

    fn state(&self) -> Health {
        if self.worker.is_running() {
            // Acquire: observe the snapshot published by the worker.
            Health::from_index(self.health.load(Ordering::Acquire))
        } else {
            Health::Unhealthy
        }
    }

    fn component(&self, name: &str) -> Component {
        let mut registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(shared) = registry.get(name).and_then(|entry| entry.owner.upgrade()) {
            return Component::new(shared);
        }

        registry.retain(|_, component| component.owner.strong_count() > 0);
        let id = next_id(&self.next_component);
        let name: Arc<str> = Arc::from(name);
        let (shared, actor) = ComponentShared::new(Arc::clone(&name), id, self.worker.sender.clone(), Arc::clone(&self.blockers));
        let owner = ComponentOwner::new(Arc::clone(&shared), Arc::clone(&self.worker));
        registry.insert(
            name,
            RegistryEntry {
                owner: Arc::downgrade(&owner),
                shared: Arc::downgrade(&shared),
            },
        );
        // Queue registration before another caller can obtain this component and publish to it.
        // A disconnected worker cannot accept registrations and already fails closed.
        let _ = self.worker.sender.send(Work::Register(id, actor));
        Component::new(owner)
    }

    fn reports(&self, filter: Filter) -> Reports {
        let components: Vec<_> = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter_map(|entry| entry.shared.upgrade())
            .collect();
        Reports::new(
            components
                .iter()
                .filter(|component| component.is_registered())
                .map(|component| component.report(filter))
                .collect(),
        )
    }
}

impl ProcessAggregate {
    pub(crate) fn new(root: Weak<AggregatorShared>) -> Self {
        Self {
            root,
            components: HashMap::new(),
            counts: [0; Health::COUNT],
        }
    }

    pub(crate) fn set(&mut self, id: u64, state: Health) {
        if let Some(previous) = self.components.insert(id, state) {
            self.counts[previous.index()] -= 1;
        }

        self.counts[state.index()] += 1;
        self.refresh(true);
    }

    pub(crate) fn remove(&mut self, id: u64, blockers: usize) {
        let previous = self
            .components
            .remove(&id)
            .expect("registration precedes the last handle's removal");
        self.counts[previous.index()] -= 1;
        if let Some(root) = self.root.upgrade() {
            // The worker reevaluates readiness below, so releasing these vetoes needs no wake.
            root.blockers.fetch_sub(blockers, Ordering::SeqCst);
        }
        self.refresh(true);
    }

    /// Even a turn that only removes an unseen publisher can release a deferred recovery.
    pub(crate) fn refresh(&self, report_changed: bool) {
        if let Some(root) = self.root.upgrade() {
            let state = Health::worst(&self.counts);
            let mut changed = report_changed;
            // SeqCst orders the readiness check with failures and recoveries across components.
            if state != Health::Healthy || root.blockers.load(Ordering::SeqCst) == 0 {
                let bits = health_bits(state);
                // Release: publish the state before waking readers.
                changed |= root.health.swap(bits, Ordering::Release) != bits;
            }
            if changed {
                root.changes.send_replace(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use futures::FutureExt;
    use futures::executor::block_on;
    use testing_aids::execute_or_terminate_process;

    use super::*;

    #[test]
    fn worker_shutdown_finishes_the_last_registered_components_report() {
        execute_or_terminate_process(|| {
            let root = Aggregator::new();
            let mut idle = root.component("idle");
            let component = root.component("live");
            let _publisher = component.publisher();
            root.shared.worker.sender.send(Work::Stop).unwrap();
            // No publisher changes the idle component. Its cursor witnesses the worker's exit.
            block_on(idle.changed());
            assert_eq!(block_on(component.report(Filter::ALL)).signal_count(Health::Healthy), 1);
        });
    }

    #[test]
    fn overlapping_component_failures_do_not_publish_a_ready_snapshot_between_turns() {
        let root = Aggregator::new();
        let (work, pending) = mpsc::channel();
        let component = |id, name| {
            let (shared, actor) = ComponentShared::new(Arc::from(name), id, work.clone(), Arc::clone(&root.shared.blockers));
            (Component::new(ComponentOwner::new(shared, Arc::clone(&root.shared.worker))), actor)
        };
        let (first, mut first_actor) = component(1, "first");
        let (second, mut second_actor) = component(2, "second");
        let mut aggregate = ProcessAggregate::new(Arc::downgrade(&root.shared));
        let mut advance = |actor: &mut _| {
            let Work::Component(next) = pending.try_recv().unwrap() else {
                panic!("expected a component turn");
            };
            next.upgrade().unwrap().process(actor, &mut aggregate);
        };

        let mut first_writer = first.publisher();
        first_writer.publish(Health::Unhealthy, [("revision", 1)]);
        let _second_healthy = second.publisher();
        advance(&mut first_actor);
        advance(&mut second_actor);
        assert_eq!(root.state(), Health::Unhealthy);

        first_writer.publish(Health::Unhealthy, [("revision", 3)]);
        let mut second_writer = second.publisher();
        second_writer.publish(Health::Unhealthy, [("revision", 4)]);
        first_writer.publish(Health::Healthy, [("revision", 5)]);
        advance(&mut first_actor);
        assert_eq!(first.state(), Health::Healthy);
        assert_eq!(root.state(), Health::Unhealthy);
        let mut monitor = root.monitor();
        // This writer was never in a snapshot. Its removal must release the root even
        // though the second component has no report change to publish.
        drop(second_writer);
        advance(&mut second_actor);
        assert_eq!(monitor.changed().now_or_never(), Some(()));
        assert_eq!(root.state(), Health::Healthy);
        drop(second.publisher());
        advance(&mut second_actor);
        assert!(
            monitor.changed().now_or_never().is_none(),
            "an unchanged report must not wake readers"
        );
    }

    #[test]
    fn process_transitions_publish_the_worst_remaining_state() {
        let mut root = Aggregator::new();
        let mut model = ProcessAggregate::new(Arc::downgrade(&root.shared));
        for (id, state, expected) in [
            (1, Some(Health::Healthy), Health::Healthy),
            // Registering and removing another healthy component changes the reports even
            // though it leaves the published state unchanged.
            (3, Some(Health::Healthy), Health::Healthy),
            (3, None, Health::Healthy),
            (2, Some(Health::Degraded), Health::Degraded),
            (1, Some(Health::Unhealthy), Health::Unhealthy),
            (1, None, Health::Degraded),
            (2, Some(Health::Healthy), Health::Healthy),
            (2, None, Health::Unhealthy),
        ] {
            if let Some(state) = state {
                model.set(id, state);
            } else {
                model.remove(id, 0);
            }
            assert_eq!(root.changed().now_or_never(), Some(()));
            assert_eq!(root.state(), expected);
        }
    }

    #[test]
    fn a_degraded_component_cannot_replace_another_components_empty_registration() {
        execute_or_terminate_process(|| {
            let root = Aggregator::new();
            let empty = root.component("required");
            let mut reporting = root.component("optional");
            let mut publisher = reporting.publisher();
            block_on(reporting.changed());

            // The component notification acknowledges its own turn, independently of the root state.
            publisher.degraded("retrying");
            block_on(reporting.changed());
            assert_eq!(reporting.state(), Health::Degraded);
            assert_eq!(root.state(), Health::Unhealthy);

            drop(empty);
            publisher.degraded("retrying again");
            block_on(reporting.changed());
            assert_eq!(root.state(), Health::Degraded);
        });
    }

    #[test]
    fn last_registration_id_can_be_allocated_once_without_wrapping() {
        let ids = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_id(&ids), u64::MAX - 1);
        if cfg!(panic = "unwind") {
            std::panic::catch_unwind(|| next_id(&ids)).unwrap_err();
        }
    }

    #[test]
    fn registration_reclaims_retired_names() {
        let root = Aggregator::new();
        drop(root.component("retired"));
        let _live = root.component("live");
        assert!(!root.shared.registry.lock().unwrap().contains_key("retired"));
    }
}
