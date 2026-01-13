// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for mocking spawners.

use crate::Spawner;

/// A mock spawner for testing that executes tasks inline.
///
/// This spawner is useful for testing code that uses [`Spawner`](crate::Spawner)
/// without requiring an actual async runtime.
///
/// # Examples
///
/// ```rust
/// use wing::testing::MockSpawner;
/// use wing::Spawner;
///
/// let spawner = MockSpawner;
/// let result = spawner.spawn(async { 42 });
/// assert_eq!(result, 42);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct MockSpawner;

impl Spawner for MockSpawner {
    fn spawn<T>(&self, work: impl Future<Output = T> + Send + 'static) -> T
    where
        T: Send + 'static,
    {
        futures::executor::block_on(work)
    }
}
