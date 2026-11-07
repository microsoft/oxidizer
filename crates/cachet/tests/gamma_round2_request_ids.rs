// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact request correlation IDs in an isolated test process.

#![cfg(feature = "test-util")]

use std::sync::{Arc, Mutex};

use cachet::{Cache, CacheEventHandler, CacheOperationEvent, CacheTierEvent, MockCache};

#[derive(Clone)]
struct Requests(Arc<Mutex<Vec<u64>>>);

impl CacheEventHandler for Requests {
    fn on_tier_event(&self, _: &CacheTierEvent<'_>) {}

    fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
        self.0
            .lock()
            .expect("test callbacks must not panic while holding the request log")
            .push(event.request_id);
    }
}

#[tokio::test]
async fn gamma_round2_isolated_requests_receive_the_exact_successive_process_ids() {
    let requests = Requests(Arc::new(Mutex::new(Vec::new())));
    let cache = Cache::builder::<i32, i32>(tick::Clock::new_frozen())
        .storage(MockCache::new())
        .event_handler(requests.clone())
        .build();
    assert!(cache.get(&7).await.unwrap().is_none());
    assert!(cache.get(&11).await.unwrap().is_none());
    assert_eq!(*requests.0.lock().unwrap(), vec![1, 2]);
}
