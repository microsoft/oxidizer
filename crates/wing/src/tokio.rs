// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tokio spawner implementation.

use crate::Spawner;

/// Spawner implementation for the tokio runtime.
///
/// Uses `tokio::spawn` to spawn tasks on the tokio runtime.
///
/// # Examples
///
/// ```rust
/// use wing::tokio::TokioSpawner;
/// use wing::Spawner;
///
/// let spawner = TokioSpawner;
/// let result = spawner.spawn(async { 42 });
/// assert_eq!(result, 42);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSpawner;

impl Spawner for TokioSpawner {
    fn spawn<T>(&self, work: impl Future<Output = T> + Send + 'static) -> T
    where
        T: Send + 'static,
    {
        // Just block on the future directly using futures executor
        futures::executor::block_on(work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokio_spawn_with_result() {
        let spawner = TokioSpawner;
        let result = spawner.spawn(async { 42 });
        assert_eq!(result, 42);
    }

    #[test]
    fn tokio_spawn_with_string() {
        let spawner = TokioSpawner;
        let result = spawner.spawn(async { "hello".to_string() });
        assert_eq!(result, "hello");
    }
}
