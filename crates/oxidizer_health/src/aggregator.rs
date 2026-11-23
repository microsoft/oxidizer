// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use performables::sync::channel::{WatchReceiver, WatchSender, watch};
use thread_aware::{Thread, ThreadAware};

use crate::component::{ComponentOwner, ComponentShared};
use crate::worker::{Work, Worker};
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
    changes: WatchReceiver<()>,
}

impl Aggregator {
    /// Starts in `Unhealthy` state with no registered components.
    #[must_use]
    pub fn new() -> Self {
        Self::from_shared(AggregatorShared::new())
    }

    fn from_shared(shared: Arc<AggregatorShared>) -> Self {
        let changes = shared.changes.subscribe();
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
            changes: self.shared.changes.subscribe(),
        }
    }

    /// Waits until the aggregate state or report data may have changed.
    ///
    /// Intermediate transitions can be coalesced. Read [`state`](Self::state) or
    /// [`reports`](Self::reports) afterwards to observe the current state.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "exclusive access keeps one waiter per notification cursor"
    )]
    pub async fn changed(&mut self) {
        wait(&self.changes).await;
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
    changes: WatchReceiver<()>,
}

impl Monitor {
    /// Waits until the aggregate state or report data may have changed.
    ///
    /// Intermediate transitions can be coalesced. Read [`state`](Self::state) or
    /// [`reports`](Self::reports) afterwards to observe the current state.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "exclusive access keeps one waiter per notification cursor"
    )]
    pub async fn changed(&mut self) {
        wait(&self.changes).await;
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

/// Waits for the next change notification on a handle's cursor.
///
/// Every public handle owns a receiver subscribed to its owner's sender, and the owner outlives
/// the handle, so the channel cannot close while this is awaited.
pub(crate) async fn wait(changes: &WatchReceiver<()>) {
    changes
        .changed_async()
        .await
        .expect("the notification sender lives as long as the handle");
}

#[expect(clippy::cast_possible_truncation, reason = "health indices are below Health::COUNT")]
const fn health_bits(state: Health) -> u8 {
    state.index() as u8
}

pub(crate) struct AggregatorShared {
    next_component: AtomicU64,
    registry: Mutex<HashMap<Arc<str>, RegistryEntry>>,
    worker: Arc<Worker>,
    health: AtomicU8,
    changes: WatchSender<()>,
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
            next_component: AtomicU64::new(0),
            registry: Mutex::new(HashMap::new()),
            worker: Worker::new(ProcessAggregate {
                root: Weak::clone(root),
                components: HashMap::new(),
                counts: [0; Health::COUNT],
            }),
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
        // Relaxed: the identifier only needs to be unique.
        let id = self.next_component.fetch_add(1, Ordering::Relaxed);
        let name: Arc<str> = Arc::from(name);
        let shared = ComponentShared::new(Arc::clone(&name), id, &self.worker);
        let owner = ComponentOwner::new(Arc::clone(&shared), Arc::clone(&self.worker));
        registry.insert(
            name,
            RegistryEntry {
                owner: Arc::downgrade(&owner),
                shared: Arc::downgrade(&shared),
            },
        );
        // Queue registration before another caller can obtain this component and publish to it.
        self.worker.send(Work::Register(id));
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
    pub(crate) fn set(&mut self, id: u64, state: Health) {
        if let Some(previous) = self.components.insert(id, state) {
            self.counts[previous.index()] -= 1;
        }
        self.counts[state.index()] += 1;
        self.notify();
    }

    pub(crate) fn remove(&mut self, id: u64) {
        let previous = self
            .components
            .remove(&id)
            .expect("registration precedes the last handle's removal");
        self.counts[previous.index()] -= 1;
        self.notify();
    }

    fn notify(&self) {
        if let Some(root) = self.root.upgrade() {
            // Release: publish the new aggregate before waking readers.
            root.health.store(health_bits(Health::worst(&self.counts)), Ordering::Release);
            root.changes.send_replace(());
        }
    }
}
