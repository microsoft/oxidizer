// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;

use futures_channel::oneshot;
use thread_aware::closure::ThreadAwareAsyncFnOnce;
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
    ///
    /// The task is provided as a [`ThreadAwareAsyncFnOnce`] whose captured data
    /// implements [`ThreadAware`], so the spawner can relocate it before execution
    /// if needed. Call [`call_once`](ThreadAwareAsyncFnOnce::call_once) to obtain
    /// the future.
    fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>);
}

/// A type-erased, heap-allocated, pinned future that returns `()`.
///
/// This is the future type that [`SpawnCustom::spawn`] implementations and
/// layer closures operate on.
pub type BoxedFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Wraps [`ThreadAware`] data, a fn pointer, and a result channel as a [`ThreadAwareAsyncFnOnce`].
///
/// Created by [`CustomSpawner::spawn_anywhere`] to bridge the typed public API
/// (`Spawner::spawn_anywhere(data, f)`) to the type-erased `SpawnCustom` trait.
struct SpawnAnywhereTask<T, D, F> {
    data: D,
    f: fn(D) -> F,
    tx: oneshot::Sender<T>,
}

impl<T: Send, D: ThreadAware, F> ThreadAware for SpawnAnywhereTask<T, D, F> {
    fn relocate(&mut self, source: Option<thread_aware::affinity::Affinity>, destination: thread_aware::affinity::Affinity) {
        self.data.relocate(source, destination);
    }
}

impl<T, D, F> ThreadAwareAsyncFnOnce<()> for SpawnAnywhereTask<T, D, F>
where
    T: Send + 'static,
    D: ThreadAware + 'static,
    F: Future<Output = T> + Send + 'static,
{
    fn call_once(self: Box<Self>) -> thread_aware::closure::BoxFuture<'static, ()> {
        let Self { data, f, tx } = *self;
        Box::pin(async move {
            let _ = tx.send(f(data).await);
        })
    }
}

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

    pub(crate) fn spawn_anywhere<T, D, F>(&self, data: D, f: fn(D) -> F) -> oneshot::Receiver<T>
    where
        T: Send + 'static,
        D: ThreadAware + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let task = Box::new(SpawnAnywhereTask { data, f, tx });
        self.spawn.spawn_anywhere(task);
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
