// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using `Merger` to prevent thundering herd when populating a cache.
//!
//! Multiple concurrent requests for the same cache key share a single execution.
//! The first request (leader) performs the work while others (followers) wait and
//! receive a clone of the result.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use uniflight::Merger;

#[tokio::main]
async fn main() {
    let merger = Merger::<String, String>::new();
    let execution_count = Arc::new(AtomicUsize::new(0));

    // Register every caller before spawning so scheduler delays cannot create
    // separate operations for the same key.
    let operations: [_; 5] = std::array::from_fn(|_| {
        let counter = Arc::clone(&execution_count);
        merger.execute("user:123", move || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            "UserData { name: Alice }".to_string()
        })
    });
    let handles = operations.map(tokio::spawn);

    // All requests complete with the same result
    for handle in handles {
        let result = handle.await.expect("task panicked");
        assert_eq!(result, Ok("UserData { name: Alice }".to_string()));
    }

    // Work executed only once despite 5 concurrent requests
    assert_eq!(execution_count.load(Ordering::SeqCst), 1);
}
