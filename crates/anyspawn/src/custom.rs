// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;

use futures_channel::oneshot;
use thread_aware::{PerCore, ThreadAware};

/// Trait for implementing custom task spawners.
///
/// Implement this trait to integrate any async runtime with [`Spawner`](crate::Spawner).
/// The spawner receives type-erased [`BoxedFuture`]s and is responsible for executing them.
///
/// Use [`Spawner::new_custom`](crate::Spawner::new_custom) to construct a [`Spawner`](crate::Spawner)
/// from a `SpawnCustom` implementation, or [`CustomSpawnerBuilder`](crate::CustomSpawnerBuilder) to
/// compose one with layer closures.
pub trait SpawnCustom: ThreadAware + Sync + 'static {
    /// Spawn a task with affinity to the current core.
    fn spawn(&self, task: BoxedFuture);
    /// Spawn a task that may run on any core.
    fn spawn_anywhere(&self, task: BoxedFuture);
}

/// A type-erased, heap-allocated, pinned future that returns `()`.
///
/// This is the future type that [`SpawnCustom`] implementations and
/// layer closures operate on.
pub type BoxedFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Internal wrapper for custom spawn functions.
#[derive(Clone, ThreadAware)]
pub(crate) struct CustomSpawner {
    spawn: thread_aware::Arc<dyn SpawnCustom, PerCore>,
    name: &'static str,
}

impl CustomSpawner {
    pub(crate) fn new<T: SpawnCustom + Clone>(name: &'static str, t: T) -> Self {
        let spawn = thread_aware::Arc::with_clone_fn(t, |x| Box::new(x.clone()) as Box<dyn SpawnCustom>);
        Self { spawn, name }
    }

    pub(crate) fn spawn<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> oneshot::Receiver<T> {
        let (tx, rx) = oneshot::channel();
        self.spawn.spawn(Box::pin(async move {
            let _ = tx.send(work.await);
        }));
        rx
    }

    pub(crate) fn spawn_anywhere<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> oneshot::Receiver<T> {
        let (tx, rx) = oneshot::channel();
        self.spawn.spawn_anywhere(Box::pin(async move {
            let _ = tx.send(work.await);
        }));
        rx
    }
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "spawn is an Arc<dyn SpawnCustom> and not useful in debug output"
)]
impl Debug for CustomSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("CustomSpawner");
        s.field("name", &self.name);
        s.finish()
    }
}
