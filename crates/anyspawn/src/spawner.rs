// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] for plugging in runtime implementations.

use std::fmt::{self, Debug};

use thread_aware::ThreadAware;

use crate::custom::{CustomSpawner, SpawnCustom};
use crate::handle::{JoinHandle, JoinHandleInner};

/// Runtime-agnostic task spawner.
///
/// `Spawner` abstracts task spawning across different async runtimes. Use the
/// built-in constructors for common runtimes or [`Spawner::new_custom`] with a
/// custom [`SpawnCustom`](crate::SpawnCustom) implementation.
///
/// # Examples
///
/// Using Tokio:
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() {
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_tokio();
/// let handle = spawner.spawn(async {
///     println!("Task running!");
/// });
/// handle.await; // Wait for task to complete
///
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
/// # async fn main() {
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_tokio();
/// let value = spawner.spawn(async { 1 + 1 }).await;
/// assert_eq!(value, 2);
/// # }
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
///
/// ## Handling Errors
///
/// Return a `Result` from the task to propagate errors:
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() {
/// use anyspawn::Spawner;
///
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
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
///
/// # Thread-Aware Support
///
/// `Spawner` implements [`ThreadAware`] and supports per-core isolation via
/// custom [`SpawnCustom`](crate::SpawnCustom) implementations. A thread-aware
/// spawner creates per-core state through cloning and calling [`relocate`](ThreadAware::relocate), enabling
/// contention-free, NUMA-friendly task dispatch. The Tokio variants do not
/// create per-core state: they ignore relocation and behave identically
/// regardless of which core they run on.
#[derive(Clone, ThreadAware)]
#[must_use]
pub struct Spawner(SpawnerKind);

#[derive(Clone, ThreadAware)]
enum SpawnerKind {
    #[cfg(feature = "tokio")]
    Tokio(#[thread_aware(skip)] Option<::tokio::runtime::Handle>),
    Custom(CustomSpawner),
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
        Self(SpawnerKind::Tokio(None))
    }

    /// Creates a spawner that uses the given Tokio runtime handle for spawning.
    ///
    /// Unlike [`new_tokio`](Self::new_tokio), this spawner does not require an
    /// ambient Tokio runtime context. It spawns tasks directly on the provided
    /// [`Handle`](::tokio::runtime::Handle).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyspawn::Spawner;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let handle = tokio::runtime::Handle::current();
    /// let spawner = Spawner::new_tokio_with_handle(handle);
    /// let result = spawner.spawn(async { 42 }).await;
    /// assert_eq!(result, 42);
    /// # }
    /// ```
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    pub fn new_tokio_with_handle(handle: ::tokio::runtime::Handle) -> Self {
        Self(SpawnerKind::Tokio(Some(handle)))
    }

    /// Creates a custom spawner from a [`SpawnCustom`](crate::SpawnCustom) implementation.
    ///
    /// The `name` identifies this spawner in [`Debug`] output.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use anyspawn::Spawner;
    ///
    /// // `MySpawner` must implement `SpawnCustom + Clone`.
    /// let spawner = Spawner::new_custom("my-runtime", MySpawner::new());
    /// ```
    pub fn new_custom<T>(name: &'static str, custom: T) -> Self
    where
        T: SpawnCustom + Clone,
    {
        Self(SpawnerKind::Custom(CustomSpawner::new(name, custom)))
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
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_tokio();
    ///
    /// // Await to get the result
    /// let value = spawner.spawn(async { 1 + 1 }).await;
    /// assert_eq!(value, 2);
    ///
    /// // Or fire-and-forget by dropping the handle
    /// let _ = spawner.spawn(async { println!("background task") });
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn spawn<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio(handle) => {
                let jh = match handle {
                    Some(h) => h.spawn(work),
                    None => ::tokio::spawn(work),
                };
                JoinHandle(JoinHandleInner::Tokio(jh))
            }
            SpawnerKind::Custom(c) => JoinHandle(JoinHandleInner::Custom(c.spawn(work))),
        }
    }

    /// Spawn a task that may run on any core, returning a [`JoinHandle`] for the result.
    ///
    /// Unlike [`spawn`](Self::spawn), this does not guarantee core affinity.
    /// The `data` must implement [`ThreadAware`](thread_aware::ThreadAware) so
    /// the spawner can relocate it before execution. The function pointer `f`
    /// receives ownership of `data` and returns a future.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_tokio();
    /// let result = spawner.spawn_anywhere(42, |x| async move { x + 1 }).await;
    /// assert_eq!(result, 43);
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn spawn_anywhere<T, D, F>(&self, data: D, f: fn(D) -> F) -> JoinHandle<T>
    where
        T: Send + 'static,
        D: ThreadAware + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio(handle) => {
                let jh = match handle {
                    Some(h) => h.spawn(f(data)),
                    None => ::tokio::spawn(f(data)),
                };
                JoinHandle(JoinHandleInner::Tokio(jh))
            }
            SpawnerKind::Custom(c) => JoinHandle(JoinHandleInner::Custom(c.spawn_anywhere(data, f))),
        }
    }

    /// Spawns a blocking (synchronous) task on the runtime.
    ///
    /// Use this for CPU-bound work or calls into blocking APIs that would
    /// otherwise stall the async executor. The closure `f` runs to completion
    /// on a thread that is allowed to block; for Tokio this is the blocking
    /// thread pool managed by [`tokio::task::spawn_blocking`].
    ///
    /// Returns a [`JoinHandle`] that can be awaited to retrieve the task's
    /// result, or dropped to run the task in fire-and-forget mode.
    ///
    /// # Panics
    ///
    /// Awaiting the returned `JoinHandle` will panic if the spawned task panics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tokio")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_tokio();
    /// let value = spawner
    ///     .spawn_blocking(|| {
    ///         // expensive synchronous work goes here
    ///         1 + 1
    ///     })
    ///     .await;
    /// assert_eq!(value, 2);
    /// # }
    /// # #[cfg(not(feature = "tokio"))]
    /// # fn main() {}
    /// ```
    pub fn spawn_blocking<T, F>(&self, f: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio(handle) => {
                let jh = match handle {
                    Some(h) => h.spawn_blocking(f),
                    None => ::tokio::task::spawn_blocking(f),
                };
                JoinHandle(JoinHandleInner::Tokio(jh))
            }
            SpawnerKind::Custom(c) => JoinHandle(JoinHandleInner::Custom(c.spawn_blocking(f))),
        }
    }
}

impl Debug for Spawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio(None) => f.debug_tuple("Spawner").field(&"tokio").finish(),
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio(Some(_)) => f.debug_tuple("Spawner").field(&"tokio(handle)").finish(),
            SpawnerKind::Custom(c) => f.debug_tuple("Spawner").field(c).finish(),
        }
    }
}
