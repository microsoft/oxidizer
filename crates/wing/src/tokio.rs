// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tokio [`Spawner`] implementation.

use crate::Spawner;

/// [`Spawner`] implementation for the Tokio runtime.
///
/// Spawns fire-and-forget tasks using `tokio::spawn`.
///
/// # Examples
///
/// Basic fire-and-forget usage:
///
/// ```rust
/// use wing::tokio::TokioSpawner;
/// use wing::Spawner;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = TokioSpawner;
/// spawner.spawn(async {
///     println!("Task running!");
/// });
/// # }
/// ```
///
/// ## Getting Results
///
/// Use a oneshot channel to retrieve a value from the spawned task:
///
/// ```rust
/// use wing::tokio::TokioSpawner;
/// use wing::Spawner;
/// use tokio::sync::oneshot;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = TokioSpawner;
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
/// use wing::tokio::TokioSpawner;
/// use wing::Spawner;
/// use tokio::sync::oneshot;
///
/// # #[tokio::main]
/// # async fn main() {
/// let spawner = TokioSpawner;
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
///
/// # Panics
///
/// - Panics if called outside of a Tokio runtime context
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSpawner;

impl Spawner for TokioSpawner {
    fn spawn<T>(&self, work: T)
    where
        T: Future<Output = ()> + Send + 'static,
    {
        ::tokio::spawn(work);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tokio_spawn_fire_and_forget() {
        let spawner = TokioSpawner;
        let (tx, rx) = tokio::sync::oneshot::channel();

        spawner.spawn(async move {
            tx.send(42).unwrap();
        });

        assert_eq!(rx.await.unwrap(), 42);
    }
}
