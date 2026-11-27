// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

use thread_aware::{Thread, ThreadAware};

use crate::aggregator::AggregatorShared;
use crate::worker::Changes;
use crate::{Health, Report};

/// A read-only handle to the process health of an [`Aggregator`](crate::Aggregator).
///
/// Each clone has an independent [`changed`](Self::changed) notification cursor.
/// A monitor keeps the process root and worker alive after all `Aggregator` handles are dropped.
/// Its state, reports, and pending change waits continue to observe component updates.
///
/// # Examples
///
/// ```
/// use health::{Aggregator, Health};
/// use observed::Sink;
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new(Sink::noop());
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
    pub(crate) fn new(shared: Arc<AggregatorShared>, changes: Changes) -> Self {
        Self { shared, changes }
    }

    /// Waits until the aggregate state or report data may have changed.
    ///
    /// Several updates can produce one notification. Read the latest [`state`](Self::state)
    /// or [`reports`](Self::reports) rather than expecting a notification for each transition.
    /// If the worker stops, each handle subscribed before it stopped observes one final change.
    /// Later waits stay pending. A new subscription after the worker stops receives no notification.
    ///
    /// The worker can call a waiting task's waker directly. A custom waker must return promptly:
    /// waiting for another update from this worker prevents it from processing that update.
    pub async fn changed(&mut self) {
        self.changes.wait(self.shared.worker()).await;
    }

    /// Returns the latest process state.
    ///
    /// The state can briefly lag component registration, removal, or publication.
    /// While a waker blocks the worker, this returns the last published state.
    /// Returns [`Health::Unhealthy`] if the worker has stopped.
    #[must_use]
    pub fn state(&self) -> Health {
        self.shared.state()
    }

    /// Returns one immutable report per registered component.
    ///
    /// Each report describes one component at one point in time. Reports for different components
    /// can come from different times. If the worker has stopped, each report describes the
    /// component's last snapshot. Use [`state`](Self::state) for current readiness. Iteration order
    /// is unspecified.
    #[expect(
        clippy::unused_async,
        reason = "the contract allows report assembly to wait for the component owners"
    )]
    pub async fn reports(&self) -> impl Iterator<Item = Report> + use<> {
        self.shared.reports()
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
