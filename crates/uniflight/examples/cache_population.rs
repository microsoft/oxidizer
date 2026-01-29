// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using `Merger` to prevent thundering herd when populating a cache.
//!
//! Multiple concurrent requests for the same cache key share a single execution.
//! The first request (leader) performs the work while others (followers) wait and
//! receive a clone of the result.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use uniflight::Merger;

#[tokio::main]
async fn main() {
    let merger = Arc::new(Merger::<String, String>::new());
    let execution_count = Arc::new(AtomicUsize::new(0));

    // Spawn 5 concurrent requests for the same key
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let merger = Arc::clone(&merger);
            let counter = Arc::clone(&execution_count);
            tokio::spawn(async move {
                merger
                    .execute("user:123", || async {
                        counter.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        "UserData { name: Alice }".to_string()
                    })
                    .await
            })
        })
        .collect();

    // All requests complete with the same result
    for handle in handles {
        let result = handle.await.expect("task panicked");
        assert_eq!(result, Ok("UserData { name: Alice }".to_string()));
    }

    // Work executed only once despite 5 concurrent requests
    assert_eq!(execution_count.load(Ordering::SeqCst), 1);
}
