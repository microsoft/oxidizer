// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Spawner`] trait for plugging in runtime implementations.

/// Trait for spawning async tasks on a runtime.
pub trait Spawner {
    /// Spawns an async task
    fn spawn<T>(&self, work: T)
    where
        T: Future<Output = ()> + Send + 'static;
}
