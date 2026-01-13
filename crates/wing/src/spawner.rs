// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] trait for plugging in runtime implementations.

/// Trait for spawning async tasks on a runtime.
pub trait Spawner {
    /// Spawns an async task and returns a value.
    fn spawn<T>(&self, work: impl Future<Output = T> + Send + 'static) -> T
    where
        T: Send + 'static;
}
