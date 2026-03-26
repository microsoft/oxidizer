// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] for plugging in runtime implementations.

use std::fmt::{self, Debug};
#[cfg(feature = "custom")]
use std::sync::Arc;

use thread_aware::{PerCore, ThreadAware};

#[cfg(feature = "custom")]
use crate::custom::{BoxedFuture, CustomSpawner};
use crate::handle::JoinHandle;
#[cfg(any(feature = "tokio", feature = "custom"))]
use crate::handle::JoinHandleInner;

/// Runtime-agnostic task spawner.
///
/// `Spawner` abstracts task spawning across different async runtimes. Use the
/// built-in constructors for common runtimes, [`Spawner::new_custom`] for a
/// simple custom closure, or [`CustomSpawnerBuilder`](crate::CustomSpawnerBuilder) for layered
/// composition with named debug output.
///
/// # Examples
///
/// Using Tokio:
///
/// ```rust
/// use anyspawn::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::new_tokio();
/// let handle = spawner.spawn(async {
///     println!("Task running!");
/// });
/// handle.await; // Wait for task to complete
///
/// # }
/// ```
///
/// ## Custom Runtime
///
/// ```rust,ignore
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_custom("threadpool", |fut| {
///     std::thread::spawn(move || futures::executor::block_on(fut));
/// });
///
/// let handle = spawner.spawn(async {
///     println!("Running on custom runtime!");
/// });
/// // handle can be awaited or dropped (fire-and-forget)
/// ```
///
/// ## Getting Results
///
/// Await the [`JoinHandle`](crate::JoinHandle) to retrieve a value from the task:
///
/// ```rust
/// use anyspawn::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::new_tokio();
/// let value = spawner.spawn(async { 1 + 1 }).await;
/// assert_eq!(value, 2);
/// # }
/// ```
///
/// ## Handling Errors
///
/// Return a `Result` from the task to propagate errors:
///
/// ```rust
/// use anyspawn::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::new_tokio();
///
/// let result = spawner
///     .spawn(async {
///         if true {
///             Ok(42)
///         } else {
///             Err("something went wrong")
///         }
///     })
///     .await;
///
/// match result {
///     Ok(value) => println!("Got {value}"),
///     Err(e) => eprintln!("Task failed: {e}"),
/// }
/// # }
/// ```
///
/// # Thread-Aware Support
///
/// `Spawner` implements [`ThreadAware`] and supports per-core isolation via
/// [`new_thread_aware`](Self::new_thread_aware). A thread-aware spawner
/// creates a **separate** inner `Spawner` for each CPU core through a
/// user-provided factory function. When the spawner is
/// [relocated](ThreadAware::relocated) to a new core, the factory is
/// re-invoked with data that has itself been relocated to the destination,
/// producing a fresh spawner tuned for that core.
///
/// This enables contention-free, NUMA-friendly task dispatch, each core
/// queues work through its own spawn function without touching shared
/// state. In contrast, the Tokio and custom variants are marked
/// `#[thread_aware(skip)]` and behave identically regardless of which core
/// they run on.
///
/// See [`new_thread_aware`](Self::new_thread_aware) for usage and examples.
#[derive(Clone, ThreadAware)]
#[must_use]
pub struct Spawner(SpawnerKind);

#[derive(Clone, ThreadAware)]
enum SpawnerKind {
    #[cfg(feature = "tokio")]
    Tokio,
    #[cfg(feature = "custom")]
    Custom(CustomSpawner),
    ThreadAware(thread_aware::Arc<Spawner, PerCore>),
}

impl Spawner {
    /// Creates a spawner that uses the Tokio runtime.
    ///
    /// Tasks are spawned via [`tokio::spawn`], which requires a Tokio runtime
    /// context at the point of spawning.
    ///
    /// # Panics
    ///
    /// [`Spawner::spawn`] will panic if called outside of a Tokio runtime context.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::Spawner;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let spawner = Spawner::new_tokio();
    /// let result = spawner.spawn(async { 42 }).await;
    /// assert_eq!(result, 42);
    /// # }
    /// ```
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    pub fn new_tokio() -> Self {
        Self(SpawnerKind::Tokio)
    }

    /// Creates a custom spawner from a closure.
    ///
    /// The `name` identifies this spawner in [`Debug`] output.
    /// The closure receives a boxed, pinned future and is responsible for
    /// spawning it on the appropriate runtime.
    ///
    /// For layer composition, use [`CustomSpawnerBuilder`](crate::CustomSpawnerBuilder).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_custom("threadpool", |fut| {
    ///     std::thread::spawn(move || futures::executor::block_on(fut));
    /// });
    /// ```
    #[cfg(feature = "custom")]
    #[cfg_attr(docsrs, doc(cfg(feature = "custom")))]
    pub fn new_custom<F>(name: &'static str, f: F) -> Self
    where
        F: Fn(BoxedFuture) + Send + Sync + 'static,
    {
        Self(SpawnerKind::Custom(CustomSpawner::new(Arc::new(f), name)))
    }

    /// Creates a thread-aware spawner with per-core isolation.
    ///
    /// Unlike [`new_custom`](Self::new_custom), which shares a single spawn
    /// function across all cores, this constructor creates a separate
    /// [`Spawner`] for each CPU core via the provided `factory`. The
    /// [`ThreadAware`] `data` is automatically relocated to each core before
    /// being passed to `factory`, enabling contention-free, NUMA-friendly
    /// spawner.
    ///
    /// # When to use this
    ///
    /// Use this when you need maximum throughput by avoiding cross-core
    /// contention. For example, if each core has its own work-stealing queue,
    /// you can pass the queue handle as `data` and have `f` build a
    /// spawner that queues directly on the local core's queue.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::Spawner;
    /// # use thread_aware::ThreadAware;
    /// # use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};
    /// # #[derive(Default, Clone)]
    /// # struct Scheduler(Option<usize>);
    /// # impl Scheduler { fn name(&self) -> String { format!("core-{}", self.0.unwrap_or(0)) } }
    /// # impl ThreadAware for Scheduler {
    /// #     fn relocated(self, _: MemoryAffinity, dest: PinnedAffinity) -> Self {
    /// #         Self(Some(dest.processor_index()))
    /// #     }
    /// # }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let scheduler = Scheduler::default();
    ///
    /// // Each core gets its own Spawner whose Scheduler carries the
    /// // destination core's processor index after relocation.
    /// let spawner = Spawner::new_thread_aware(
    ///     scheduler,
    ///     |scheduler| {
    ///         Spawner::new_custom("per-core-tokio", move |fut| {
    ///             println!("{}: spawning", scheduler.name());
    ///             tokio::spawn(fut);
    ///         })
    ///     },
    /// );
    ///
    /// let result = spawner.spawn(async { 1 + 1 }).await;
    /// assert_eq!(result, 2);
    /// # }
    /// ```
    pub fn new_thread_aware<D>(data: D, factory: fn(D) -> Self) -> Self
    where
        D: ThreadAware + Send + Sync + Clone + 'static,
    {
        let arc = thread_aware::Arc::new_with(data, factory);
        Self(SpawnerKind::ThreadAware(arc))
    }

    /// Spawns an async task on the runtime.
    ///
    /// Returns a [`JoinHandle`] that can be awaited to retrieve the task's result,
    /// or dropped to run the task in fire-and-forget mode.
    ///
    /// # Panics
    ///
    /// Awaiting the returned `JoinHandle` will panic if the spawned task panics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::Spawner;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let spawner = Spawner::new_tokio();
    ///
    /// // Await to get the result
    /// let value = spawner.spawn(async { 1 + 1 }).await;
    /// assert_eq!(value, 2);
    ///
    /// // Or fire-and-forget by dropping the handle
    /// let _ = spawner.spawn(async { println!("background task") });
    /// # }
    /// ```
    pub fn spawn<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio => JoinHandle(JoinHandleInner::Tokio(::tokio::spawn(work))),
            #[cfg(feature = "custom")]
            SpawnerKind::Custom(c) => JoinHandle(JoinHandleInner::Custom(c.call(work))),
            SpawnerKind::ThreadAware(ta) => ta.spawn(work),
        }
    }
}

impl Debug for Spawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio => f.debug_tuple("Spawner").field(&"tokio").finish(),
            #[cfg(feature = "custom")]
            SpawnerKind::Custom(c) => f.debug_tuple("Spawner").field(c).finish(),
            SpawnerKind::ThreadAware(_) => f.debug_tuple("Spawner").field(&"thread_aware").finish(),
        }
    }
}
