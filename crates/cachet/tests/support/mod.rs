// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Mutex;

use cachet::{CacheEventHandler, CacheOperationEvent, CacheTierEvent};
use futures::channel::oneshot;

pub(super) struct Completion(pub(super) Mutex<Option<oneshot::Sender<()>>>);
impl CacheEventHandler for Completion {
    fn on_tier_event(&self, _: &CacheTierEvent<'_>) {}
    fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
        if event.operation == "cache.refresh" {
            self.0.lock().unwrap().take().unwrap().send(()).unwrap();
        }
    }
}
