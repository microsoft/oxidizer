// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use futures_channel::oneshot;

/// A type-erased, heap-allocated, pinned future that returns `()`.
///
/// This is the future type that [`CustomSpawnerBuilder`](crate::CustomSpawnerBuilder)
/// layers and spawn functions operate on.
pub type BoxedFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type SpawnFn = dyn Fn(BoxedFuture) + Send + Sync;

/// Internal wrapper for custom spawn functions.
#[derive(Clone)]
pub(crate) struct CustomSpawner {
    spawn_fn: Arc<SpawnFn>,
    name: &'static str,
    layer_names: Arc<[&'static str]>,
}

impl CustomSpawner {
    pub(crate) fn new(spawn_fn: Arc<SpawnFn>, name: &'static str, layer_names: Arc<[&'static str]>) -> Self {
        Self {
            spawn_fn,
            name,
            layer_names,
        }
    }

    pub(crate) fn call<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> oneshot::Receiver<T> {
        let (tx, rx) = oneshot::channel();
        (self.spawn_fn)(Box::pin(async move {
            let _ = tx.send(work.await);
        }));
        rx
    }
}

#[expect(clippy::missing_fields_in_debug, reason = "spawn_fn is a closure and not useful in debug output")]
impl Debug for CustomSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("CustomSpawner");
        s.field("name", &self.name);
        if !self.layer_names.is_empty() {
            s.field("layers", &self.layer_names);
        }
        s.finish()
    }
}
