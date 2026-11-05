// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::task::Scheduler;
#[cfg(any(test, feature = "observed"))]
use observed::Sink;

/// Dependencies for an Arty spawner.
///
/// The scheduler selects the worker association and does not keep its runtime
/// running. Pass these dependencies to [`Spawner::new_arty`](crate::Spawner::new_arty).
/// See the [Arty example](crate#using-arty).
#[derive(Clone, Debug)]
#[must_use]
pub struct ArtyDeps {
    pub(crate) scheduler: Scheduler,
    #[cfg(any(test, feature = "observed"))]
    pub(crate) sink: Option<Sink>,
}

impl ArtyDeps {
    /// Creates dependencies for the given worker scheduler.
    ///
    /// See the [Arty example](crate#using-arty).
    pub fn new(scheduler: Scheduler) -> Self {
        Self {
            scheduler,
            #[cfg(any(test, feature = "observed"))]
            sink: None,
        }
    }

    /// Enables enrichment transfer from `sink` for every spawn call.
    ///
    /// Enrichment is context attached to events. The spawner captures it when
    /// work is spawned, not when these dependencies are created or cloned.
    /// Async tasks apply it on each poll, and blocking tasks apply it during
    /// the closure call. `spawn_anywhere` also applies it while its factory
    /// creates the future, after Arty relocates the data.
    ///
    /// Calling this method again replaces the sink.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(all(feature = "arty", feature = "observed"))]
    /// # fn configure(scheduler: arty::task::Scheduler, sink: observed::Sink) {
    /// use anyspawn::{ArtyDeps, Spawner};
    ///
    /// let spawner = Spawner::new_arty(ArtyDeps::new(scheduler).observed(sink));
    /// # let _ = spawner;
    /// # }
    /// ```
    #[cfg(any(test, feature = "observed"))]
    pub fn observed(mut self, sink: Sink) -> Self {
        self.sink = Some(sink);
        self
    }
}

impl From<Scheduler> for ArtyDeps {
    fn from(scheduler: Scheduler) -> Self {
        Self::new(scheduler)
    }
}

impl From<&Scheduler> for ArtyDeps {
    fn from(scheduler: &Scheduler) -> Self {
        Self::new(scheduler.clone())
    }
}
