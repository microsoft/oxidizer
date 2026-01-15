// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] for plugging in runtime implementations.

use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use futures_channel::oneshot;

type BoxedFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type SpawnFn = dyn Fn(BoxedFuture) + Send + Sync;

/// Runtime-agnostic task spawner.
///
/// `Spawner` abstracts task spawning across different async runtimes. Use the
/// built-in constructors for common runtimes, or [`Spawner::custom`] for custom
/// implementations.
///
/// # Examples
///
/// Using Tokio:
///
/// ```rust
/// use arty::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::tokio();
/// spawner.spawn(async {
///     println!("Task running!");
/// });
/// # }
/// ```
///
/// ## Custom Runtime
///
/// ```rust
/// use arty::Spawner;
///
/// # fn main() {
/// let spawner = Spawner::custom(|fut| {
///     std::thread::spawn(move || futures::executor::block_on(fut));
/// });
///
/// spawner.spawn(async {
///     println!("Running on custom runtime!");
/// });
/// # }
/// ```
///
/// ## Getting Results
///
/// Use [`run`](Spawner::run) to retrieve a value from the task:
///
/// ```rust
/// use arty::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::tokio();
/// let value = spawner.run(async { 1 + 1 }).await;
/// assert_eq!(value, 2);
/// # }
/// ```
///
/// ## Handling Errors
///
/// Return a `Result` from the task to propagate errors:
///
/// ```rust
/// use arty::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::tokio();
///
/// let result = spawner
///     .run(async {
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
    /// use arty::Spawner;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let spawner = Spawner::tokio();
    /// spawner.spawn(async {
    ///     println!("Running on Tokio!");
    /// });
    /// # }
    /// ```
    #[must_use]
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    pub fn tokio() -> Self {
        Self(SpawnerKind::Tokio)
    }

    /// Creates a custom spawner from a closure.
    ///
    /// The closure receives a boxed, pinned future and is responsible for
    /// spawning it on the appropriate runtime.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use arty::Spawner;
    ///
    /// let spawner = Spawner::custom(|fut| {
    ///     std::thread::spawn(move || futures::executor::block_on(fut));
    /// });
    /// ```
    pub fn custom<F>(f: F) -> Self
    where
        F: Fn(BoxedFuture) + Send + Sync + 'static,
    {
        Self(SpawnerKind::Custom(CustomSpawner(Arc::new(f))))
    }

    /// Spawns an async task on the runtime.
    ///
    /// The task runs independently and its result is discarded. Use
    /// [`run`](Self::run) to retrieve results.
    pub fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        match &self.0 {
            #[cfg(feature = "tokio")]
            SpawnerKind::Tokio => {
                ::tokio::spawn(work);
            }
            SpawnerKind::Custom(c) => (c.0)(Box::pin(work)),
        }
    }

    /// Runs an async task and returns its result.
    ///
    /// The task runs independently.
    ///
    /// # Panics
    ///
    /// Panics if the spawned task panics before producing a result.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use arty::Spawner;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let spawner = Spawner::tokio();
    /// let result = spawner.run(async { 1 + 1 }).await;
    /// assert_eq!(result, 2);
    /// # }
    /// ```
    pub async fn run<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> T {
        let (tx, rx) = oneshot::channel();
        self.spawn(async move {
            let _ = tx.send(work.await);
        });
        rx.await.expect("spawned task panicked")
    }
}

/// Internal wrapper for custom spawn functions.
#[derive(Clone)]
pub(crate) struct CustomSpawner(Arc<SpawnFn>);

impl Debug for CustomSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSpawner").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn tokio_spawn_fire_and_forget() {
        let spawner = Spawner::tokio();
        let (tx, rx) = tokio::sync::oneshot::channel();

        spawner.spawn(async move {
            tx.send(42).unwrap();
        });

        assert_eq!(rx.await.unwrap(), 42);
    }

    #[test]
    fn custom_spawn() {
        let spawner = Spawner::custom(|fut| {
            std::thread::spawn(move || futures::executor::block_on(fut));
        });

        let (tx, rx) = std::sync::mpsc::channel();

        spawner.spawn(async move {
            tx.send(42).unwrap();
        });

        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn tokio_run() {
        let spawner = Spawner::tokio();
        let result = spawner.run(async { 42 }).await;
        assert_eq!(result, 42);
    }

    #[test]
    fn custom_run() {
        let spawner = Spawner::custom(|fut| {
            std::thread::spawn(move || futures::executor::block_on(fut));
        });

        let result = futures::executor::block_on(spawner.run(async { 42 }));
        assert_eq!(result, 42);
    }
}
