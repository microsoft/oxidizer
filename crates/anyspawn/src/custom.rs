// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use futures_channel::oneshot;

pub(crate) type BoxedFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type SpawnFn = dyn Fn(BoxedFuture) + Send + Sync;

/// Internal wrapper for custom spawn functions.
#[derive(Clone)]
pub(crate) struct CustomSpawner(pub(crate) Arc<SpawnFn>);

impl CustomSpawner {
    pub(crate) fn call<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> oneshot::Receiver<T> {
        let (tx, rx) = oneshot::channel();
        (self.0)(Box::pin(async move {
            let _ = tx.send(work.await);
        }));
        rx
    }
}

impl Debug for CustomSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSpawner").finish_non_exhaustive()
    }
}
