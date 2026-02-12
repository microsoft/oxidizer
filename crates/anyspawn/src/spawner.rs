// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] for plugging in runtime implementations.

use std::fmt::Debug;
#[cfg(feature = "custom")]
use std::sync::Arc;

use crate::handle::JoinHandle;
#[cfg(any(feature = "tokio", feature = "custom"))]
use crate::handle::JoinHandleInner;

#[cfg(feature = "custom")]
use crate::custom::{BoxedFuture, CustomSpawner};

/// Runtime-agnostic task spawner.
///
/// `Spawner` abstracts task spawning across different async runtimes. Use the
/// built-in constructors for common runtimes, or [`Spawner::new_custom`] for custom
/// implementations.
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
/// # }
/// ```
///
/// ## Custom Runtime
///
/// ```rust,ignore
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_custom(|fut| {
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
///         if true { Ok(42) } else { Err("something went wrong") }
///     })
///     .await;
///
/// match result {
///     Ok(value) => println!("Got {value}"),
///     Err(e) => eprintln!("Task failed: {e}"),
/// }
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Spawner(SpawnerKind);

#[derive(Debug, Clone)]
enum SpawnerKind {
    #[cfg(feature = "tokio")]
    Tokio,
    #[cfg(feature = "custom")]
    Custom(CustomSpawner),
}

impl Spawner {
    /// Creates a spawner that uses the Tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a Tokio runtime context.
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
    #[must_use]
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    pub fn new_tokio() -> Self {
        Self(SpawnerKind::Tokio)
    }

    /// Creates a custom spawner from a closure.
    ///
    /// The closure receives a boxed, pinned future and is responsible for
    /// spawning it on the appropriate runtime.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use anyspawn::Spawner;
    ///
    /// let spawner = Spawner::new_custom(|fut| {
    ///     std::thread::spawn(move || futures::executor::block_on(fut));
    /// });
    /// ```
    #[cfg(feature = "custom")]
    #[cfg_attr(docsrs, doc(cfg(feature = "custom")))]
    pub fn new_custom<F>(f: F) -> Self
    where
        F: Fn(BoxedFuture) + Send + Sync + 'static,
    {
        Self(SpawnerKind::Custom(CustomSpawner(Arc::new(f))))
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
        }
    }
}
