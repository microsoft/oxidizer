// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tokio [`Spawner`] implementation.

use crate::Spawner;

/// [`Spawner`] implementation for the Tokio runtime.
///
/// Spawns tasks using `tokio::spawn` and blocks on the result.
/// This requires being called from within a Tokio **multi-threaded** runtime context.
///
/// # Examples
///
/// ```rust
/// use wing::tokio::TokioSpawner;
/// use wing::Spawner;
///
/// let rt = tokio::runtime::Builder::new_multi_thread()
///     .enable_all()
///     .build()
///     .unwrap();
///
/// rt.block_on(async {
///     let spawner = TokioSpawner;
///     let result = spawner.spawn(async { 42 });
///     assert_eq!(result, 42);
/// });
/// ```
///
/// # Panics
///
/// - Panics if called outside of a Tokio runtime context
/// - Panics if called from within a single-threaded (`current_thread`) runtime
/// - Panics if the spawned task panics
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSpawner;

impl Spawner for TokioSpawner {
    fn spawn<T>(&self, work: impl Future<Output = T> + Send + 'static) -> T
    where
        T: Send + 'static,
    {
        let handle = ::tokio::spawn(work);
        ::tokio::task::block_in_place(|| {
            ::tokio::runtime::Handle::current()
                .block_on(handle)
                .expect("spawned task panicked")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn tokio_spawn_with_result() {
        let spawner = TokioSpawner;
        let result = spawner.spawn(async { 42 });
        assert_eq!(result, 42);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tokio_spawn_with_string() {
        let spawner = TokioSpawner;
        let result = spawner.spawn(async { "hello".to_string() });
        assert_eq!(result, "hello");
    }
}
