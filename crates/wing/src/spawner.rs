// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] enum for plugging in runtime implementations.

use std::pin::Pin;
use std::sync::Arc;

type BoxedFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type SpawnFn = dyn Fn(BoxedFuture) + Send + Sync;

/// Runtime-agnostic task spawner.
///
/// `Spawner` abstracts task spawning across different async runtimes. Use the
/// built-in variants for common runtimes, or [`Spawner::new_custom`] for custom
/// implementations.
///
/// # Examples
///
/// Using Tokio:
///
/// ```rust
/// use wing::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::Tokio;
/// spawner.spawn(async {
///     println!("Task running!");
/// });
/// # }
/// ```
///
/// ## Custom Runtime
///
/// ```rust
/// use wing::Spawner;
///
/// # fn main() {
/// let spawner = Spawner::new_custom(|fut| {
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
/// Use a oneshot channel to retrieve a value from the spawned task:
///
/// ```rust
/// use wing::Spawner;
/// use tokio::sync::oneshot;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::Tokio;
/// let (tx, rx) = oneshot::channel();
///
/// spawner.spawn(async move {
///     let result = 1 + 1;
///     let _ = tx.send(result);
/// });
///
/// let value = rx.await.unwrap();
/// assert_eq!(value, 2);
/// # }
/// ```
///
/// ## Handling Errors
///
/// Send a `Result` through the channel to propagate errors:
///
/// ```rust
/// use wing::Spawner;
/// use tokio::sync::oneshot;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = Spawner::Tokio;
/// let (tx, rx) = oneshot::channel::<Result<i32, &'static str>>();
///
/// spawner.spawn(async move {
///     let result = if true { Ok(42) } else { Err("something went wrong") };
///     let _ = tx.send(result);
/// });
///
/// match rx.await.unwrap() {
///     Ok(value) => println!("Got {value}"),
///     Err(e) => eprintln!("Task failed: {e}"),
/// }
/// # }
/// ```
#[derive(Debug)]
pub enum Spawner {
    /// Spawns tasks using [`tokio::spawn`].
    ///
    /// # Panics
    ///
    /// Panics if called outside of a Tokio runtime context.
    #[cfg(feature = "tokio")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
    Tokio,

    /// Custom spawner implementation.
    ///
    /// Created via [`Spawner::new_custom`].
    Custom(CustomSpawner),
}

impl Spawner {
    /// Creates a custom spawner from a closure.
    ///
    /// The closure receives a boxed, pinned future and is responsible for
    /// spawning it on the appropriate runtime.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use wing::Spawner;
    ///
    /// let spawner = Spawner::new_custom(|fut| {
    ///     std::thread::spawn(move || futures::executor::block_on(fut));
    /// });
    /// ```
    pub fn new_custom<F>(f: F) -> Self
    where
        F: Fn(BoxedFuture) + Send + Sync + 'static,
    {
        Spawner::Custom(CustomSpawner(Arc::new(f)))
    }

    /// Spawns an async task on the runtime.
    ///
    /// The task runs independently and its result is discarded. Use a channel
    /// to retrieve results if needed.
    pub fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        match self {
            #[cfg(feature = "tokio")]
            Spawner::Tokio => {
                ::tokio::spawn(work);
            }
            Spawner::Custom(c) => (c.0)(Box::pin(work)),
        }
    }
}

/// Internal wrapper for custom spawn functions.
pub struct CustomSpawner(Arc<SpawnFn>);

impl std::fmt::Debug for CustomSpawner {
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
        let spawner = Spawner::Tokio;
        let (tx, rx) = tokio::sync::oneshot::channel();

        spawner.spawn(async move {
            tx.send(42).unwrap();
        });

        assert_eq!(rx.await.unwrap(), 42);
    }

    #[test]
    fn custom_spawn() {
        let spawner = Spawner::new_custom(|fut| {
            std::thread::spawn(move || futures::executor::block_on(fut));
        });

        let (tx, rx) = std::sync::mpsc::channel();

        spawner.spawn(async move {
            tx.send(42).unwrap();
        });

        assert_eq!(rx.recv().unwrap(), 42);
    }
}
