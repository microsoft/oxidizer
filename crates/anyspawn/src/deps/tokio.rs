// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(test, feature = "observed"))]
use observed::Sink;
use tokio::runtime::Handle;

/// Dependencies for a Tokio spawner.
///
/// [`new`](Self::new) and [`Default`] select the runtime active at each spawn
/// call. [`with_handle`](Self::with_handle) selects an explicit runtime.
/// Pass these dependencies to [`Spawner::new_tokio`](crate::Spawner::new_tokio).
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct TokioDeps {
    pub(crate) handle: Option<Handle>,
    #[cfg(any(test, feature = "observed"))]
    pub(crate) sink: Option<Sink>,
}

impl TokioDeps {
    /// Creates dependencies that use the runtime active at each spawn call.
    ///
    /// Spawning through the resulting spawner outside a Tokio runtime panics.
    /// Construction itself does not require an active runtime.
    /// See the [Tokio example](crate#using-tokio).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates dependencies that spawn tasks on the given runtime handle.
    ///
    /// Spawning does not require an ambient Tokio runtime context.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # fn configure(handle: tokio::runtime::Handle) {
    /// use anyspawn::{Spawner, TokioDeps};
    ///
    /// let spawner = Spawner::new_tokio(TokioDeps::with_handle(handle));
    /// # let _ = spawner;
    /// # }
    /// ```
    pub fn with_handle(handle: Handle) -> Self {
        Self {
            handle: Some(handle),
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
    /// creates the future.
    ///
    /// Calling this method again replaces the sink. Runtime selection does not
    /// change, so this also works with [`Self::with_handle`].
    /// See the [enrichment example](crate#observed-enrichment).
    #[cfg(any(test, feature = "observed"))]
    pub fn observed(mut self, sink: Sink) -> Self {
        self.sink = Some(sink);
        self
    }
}

impl From<Handle> for TokioDeps {
    fn from(handle: Handle) -> Self {
        Self::with_handle(handle)
    }
}

impl From<&Handle> for TokioDeps {
    fn from(handle: &Handle) -> Self {
        Self::with_handle(handle.clone())
    }
}
