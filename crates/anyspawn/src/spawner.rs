// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] for dispatching tasks to Arty or Tokio.

use std::fmt::{self, Debug};

#[cfg(any(test, feature = "observed"))]
use observed::Sink;
#[cfg(any(test, feature = "observed"))]
use observed::enrichment::EnrichFutureExt;
use thread_aware::ThreadAware;

use crate::handle::JoinHandle;
#[cfg(any(test, feature = "arty", feature = "tokio"))]
use crate::handle::JoinHandleInner;

/// A task spawner for Arty and Tokio.
///
/// Enable `arty` or `tokio` to construct a spawner for that runtime.
/// Without either feature, this type remains available but cannot be constructed.
///
/// # Examples
///
/// Using Tokio:
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() -> Result<(), anyspawn::JoinError> {
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_tokio(None);
/// let handle = spawner.spawn(async {
///     println!("Task running!");
/// });
/// handle.await?; // Wait for task to complete
///
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
///
/// ## Getting Results
///
/// Await the [`JoinHandle`](crate::JoinHandle) to retrieve a value from the task:
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() -> Result<(), anyspawn::JoinError> {
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_tokio(None);
/// let value = spawner.spawn(async { 1 + 1 }).await?;
/// assert_eq!(value, 2);
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
///
/// ## Handling Errors
///
/// Awaiting returns `Result<T, JoinError>`. A task that also returns a `Result`
/// produces a nested result. Handle the join error before the task's own error:
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() -> Result<(), anyspawn::JoinError> {
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_tokio(None);
///
/// let result = spawner
///     .spawn(async {
///         if true {
///             Ok(42)
///         } else {
///             Err("something went wrong")
///         }
///     })
///     .await?;
///
/// match result {
///     Ok(value) => println!("Got {value}"),
///     Err(e) => eprintln!("Task failed: {e}"),
/// }
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
///
/// # Thread-Aware Support
///
/// `Spawner` implements [`ThreadAware`]. Arty spawners follow their scheduler's
/// worker association when [`relocate`](ThreadAware::relocate) is called.
/// Tokio spawners ignore relocation and keep the same runtime selection.
#[derive(Clone, ThreadAware)]
#[must_use]
pub struct Spawner {
    kind: SpawnerKind,
    #[cfg(any(test, feature = "observed"))]
    sink: Option<Sink>,
}

#[derive(Clone, ThreadAware)]
enum SpawnerKind {
    #[cfg(any(test, feature = "arty"))]
    Arty(::arty::task::Scheduler),
    #[cfg(any(test, feature = "tokio"))]
    Tokio(#[thread_aware(skip)] Option<::tokio::runtime::Handle>),
}

/// Configures a [`Spawner`] for Arty or Tokio.
///
/// Start with `Spawner::builder_arty` or `Spawner::builder_tokio`.
/// A builder without a sink does not transfer enrichment to spawned work.
#[derive(Clone, Debug)]
#[must_use]
pub struct SpawnerBuilder {
    spawner: Spawner,
}

impl SpawnerBuilder {
    /// Enables enrichment transfer from `sink` for every spawn call.
    ///
    /// Enrichment is context attached to events. The spawner captures it when
    /// work is spawned, not when the builder is created or cloned.
    /// Async tasks apply it on each poll, and blocking tasks apply it during
    /// the closure call. `spawn_anywhere` also applies it while its factory
    /// creates the future, after Arty relocates the data.
    ///
    /// Calling this method again replaces the sink.
    /// See the [enrichment example](crate#observed-enrichment).
    #[cfg(any(test, feature = "observed"))]
    pub fn observed(mut self, sink: Sink) -> Self {
        self.spawner.sink = Some(sink);
        self
    }

    /// Builds the spawner with the selected runtime and sink.
    ///
    /// This does not start a runtime or require an active runtime.
    pub fn build(self) -> Spawner {
        self.spawner
    }
}

impl Spawner {
    /// Creates a Tokio spawner with an optional runtime handle.
    ///
    /// Pass `None` to use the runtime active at each spawn call.
    /// Pass a handle or `Some(handle)` to select an explicit runtime.
    /// Construction does not require an active runtime.
    /// Use [`Self::builder_tokio`] to configure enrichment transfer.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # fn configure(handle: tokio::runtime::Handle) {
    /// use anyspawn::Spawner;
    ///
    /// let explicit = Spawner::new_tokio(handle);
    /// let ambient = Spawner::new_tokio(None);
    /// # let _ = (explicit, ambient);
    /// # }
    /// ```
    #[cfg(any(test, feature = "tokio"))]
    pub fn new_tokio(handle: impl Into<Option<::tokio::runtime::Handle>>) -> Self {
        Self::builder_tokio(handle).build()
    }

    /// Configures a Tokio spawner with an optional runtime handle.
    ///
    /// Pass `None` to use the runtime active at each spawn call.
    /// Pass a handle or `Some(handle)` to select an explicit runtime.
    /// Construction does not require an active runtime.
    /// See [`Self::new_tokio`] for a runtime selection example.
    #[cfg(any(test, feature = "tokio"))]
    pub fn builder_tokio(handle: impl Into<Option<::tokio::runtime::Handle>>) -> SpawnerBuilder {
        SpawnerBuilder {
            spawner: Self {
                kind: SpawnerKind::Tokio(handle.into()),
                #[cfg(any(test, feature = "observed"))]
                sink: None,
            },
        }
    }

    /// Creates an Arty spawner from a worker scheduler.
    ///
    /// Use [`Self::builder_arty`] to configure enrichment transfer.
    /// Tasks retain the scheduler's worker association. Cloning and relocation
    /// follow the scheduler's semantics, and the spawner does not keep its
    /// runtime running.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "arty")]
    /// # fn configure(scheduler: arty::task::Scheduler) {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_arty(scheduler);
    /// # let _ = spawner;
    /// # }
    /// ```
    #[cfg(any(test, feature = "arty"))]
    pub fn new_arty(scheduler: ::arty::task::Scheduler) -> Self {
        Self::builder_arty(scheduler).build()
    }

    /// Configures an Arty spawner from a worker scheduler.
    ///
    /// The spawner retains the scheduler's worker association
    /// but does not keep its runtime running.
    /// See [`Self::new_arty`] for a scheduler selection example.
    #[cfg(any(test, feature = "arty"))]
    pub fn builder_arty(scheduler: ::arty::task::Scheduler) -> SpawnerBuilder {
        SpawnerBuilder {
            spawner: Self {
                kind: SpawnerKind::Arty(scheduler),
                #[cfg(any(test, feature = "observed"))]
                sink: None,
            },
        }
    }

    /// Spawns an async task on the runtime.
    ///
    /// Returns a [`JoinHandle`] that can be awaited to retrieve the task's result,
    /// or dropped to run the task in fire-and-forget mode.
    ///
    /// # Errors
    ///
    /// The returned handle yields [`JoinError`](crate::JoinError) if the runtime
    /// reports a task panic, cancellation, or shutdown rejection.
    ///
    /// # Panics
    ///
    /// Tokio spawners without an explicit handle panic when spawning outside a
    /// Tokio runtime.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), anyspawn::JoinError> {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_tokio(None);
    ///
    /// // Await to get the result
    /// let value = spawner.spawn(async { 1 + 1 }).await?;
    /// assert_eq!(value, 2);
    ///
    /// // Or fire-and-forget by dropping the handle
    /// let _ = spawner.spawn(async { println!("background task") });
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn spawn<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        #[cfg(any(test, feature = "observed"))]
        if let Some(sink) = &self.sink {
            return self.spawn_inner(work.attach(sink.transfer_context()));
        }
        self.spawn_inner(work)
    }

    fn spawn_inner<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        #[cfg(not(any(test, feature = "arty", feature = "tokio")))]
        let _ = work;

        match self.kind {
            #[cfg(any(test, feature = "arty"))]
            SpawnerKind::Arty(ref scheduler) => JoinHandle::new(JoinHandleInner::Arty(scheduler.spawn(move |_| work))),
            #[cfg(any(test, feature = "tokio"))]
            SpawnerKind::Tokio(ref handle) => {
                let jh = match handle {
                    Some(h) => h.spawn(work),
                    None => ::tokio::spawn(work),
                };
                JoinHandle::new(JoinHandleInner::Tokio(jh))
            }
        }
    }

    /// Spawns a task that may run on any runtime thread, returning a [`JoinHandle`] for the result.
    ///
    /// Unlike [`spawn`](Self::spawn), this does not guarantee execution on the current runtime thread.
    /// The `data` must implement [`ThreadAware`](thread_aware::ThreadAware) so
    /// the spawner can relocate it before execution. The function pointer `f`
    /// receives ownership of `data` and returns a future. The result must also
    /// implement `ThreadAware`.
    ///
    /// # Errors
    ///
    /// The returned handle reports the same errors as [`Self::spawn`].
    ///
    /// # Panics
    ///
    /// Tokio spawners without an explicit handle require an active Tokio runtime.
    /// Tokio calls `f` before spawning, so a panic in `f` propagates to the caller.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), anyspawn::JoinError> {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_tokio(None);
    /// let result = spawner.spawn_anywhere(42, |x| async move { x + 1 }).await?;
    /// assert_eq!(result, 43);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn spawn_anywhere<T, D, F>(&self, data: D, f: fn(D) -> F) -> JoinHandle<T>
    where
        T: ThreadAware + 'static,
        D: ThreadAware + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        #[cfg(any(test, feature = "observed"))]
        if let Some(sink) = &self.sink {
            return self.spawn_anywhere_inner((data, f, sink.transfer_context()), |(data, f, transfer)| {
                let future = {
                    let _guard = transfer.apply_current_thread();
                    f(data)
                };
                future.attach(transfer)
            });
        }
        self.spawn_anywhere_inner(data, f)
    }

    fn spawn_anywhere_inner<T, D, F>(&self, data: D, f: fn(D) -> F) -> JoinHandle<T>
    where
        T: ThreadAware + 'static,
        D: ThreadAware + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        #[cfg(not(any(test, feature = "arty", feature = "tokio")))]
        let _ = (data, f);

        match self.kind {
            #[cfg(any(test, feature = "arty"))]
            SpawnerKind::Arty(ref scheduler) => JoinHandle::new(JoinHandleInner::Arty(scheduler.spawn_anywhere(data, f))),
            #[cfg(any(test, feature = "tokio"))]
            SpawnerKind::Tokio(ref handle) => {
                let jh = match handle {
                    Some(h) => h.spawn(f(data)),
                    None => ::tokio::spawn(f(data)),
                };
                JoinHandle::new(JoinHandleInner::Tokio(jh))
            }
        }
    }

    /// Spawns a blocking (synchronous) task on the runtime.
    ///
    /// Use this for CPU-bound work or calls into blocking APIs that would
    /// otherwise stall the async executor. The closure `f` runs to completion
    /// on a thread that is allowed to block; for Tokio this is the blocking
    /// thread pool managed by Tokio.
    ///
    /// Returns a [`JoinHandle`] that can be awaited to retrieve the task's
    /// result, or dropped to run the task in fire-and-forget mode.
    ///
    /// # Errors
    ///
    /// The returned handle reports the same errors as [`Self::spawn`].
    ///
    /// # Panics
    ///
    /// The same panic conditions as [`Self::spawn`] apply.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), anyspawn::JoinError> {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_tokio(None);
    /// let value = spawner
    ///     .spawn_blocking(|| {
    ///         // expensive synchronous work goes here
    ///         1 + 1
    ///     })
    ///     .await?;
    /// assert_eq!(value, 2);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn spawn_blocking<T, F>(&self, f: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        #[cfg(any(test, feature = "observed"))]
        if let Some(sink) = &self.sink {
            let transfer = sink.transfer_context();
            return self.spawn_blocking_inner(move || {
                let _guard = transfer.apply_current_thread();
                f()
            });
        }
        self.spawn_blocking_inner(f)
    }

    fn spawn_blocking_inner<T, F>(&self, f: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        #[cfg(not(any(test, feature = "arty", feature = "tokio")))]
        let _ = f;

        match self.kind {
            #[cfg(any(test, feature = "arty"))]
            SpawnerKind::Arty(ref scheduler) => JoinHandle::new(JoinHandleInner::Arty(scheduler.spawn_blocking(f))),
            #[cfg(any(test, feature = "tokio"))]
            SpawnerKind::Tokio(ref handle) => {
                let jh = match handle {
                    Some(h) => h.spawn_blocking(f),
                    None => ::tokio::task::spawn_blocking(f),
                };
                JoinHandle::new(JoinHandleInner::Tokio(jh))
            }
        }
    }
}

impl Debug for Spawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(not(any(test, feature = "arty", feature = "tokio")))]
        {
            let _ = f;
            match self.kind {}
        }
        #[cfg(any(test, feature = "arty", feature = "tokio"))]
        {
            let mut debug = f.debug_tuple("Spawner");
            match &self.kind {
                #[cfg(any(test, feature = "arty"))]
                SpawnerKind::Arty(_) => debug.field(&"arty"),
                #[cfg(any(test, feature = "tokio"))]
                SpawnerKind::Tokio(None) => debug.field(&"tokio"),
                #[cfg(any(test, feature = "tokio"))]
                SpawnerKind::Tokio(Some(_)) => debug.field(&"tokio(handle)"),
            };
            #[cfg(any(test, feature = "observed"))]
            if let Some(sink) = &self.sink {
                debug.field(sink);
            }
            debug.finish()
        }
    }
}
