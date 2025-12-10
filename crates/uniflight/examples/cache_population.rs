// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using `UniFlight` to prevent thundering herd when populating a cache.
//!
//! Multiple concurrent requests for the same cache key will share a single execution,
//! with the first request (leader) performing the work and subsequent requests (followers)
//! receiving a copy of the result.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use uniflight::UniFlight;

#[tokio::main]
async fn main() {
    // Create a shared UniFlight instance for cache operations
    let cache_group = Arc::new(UniFlight::<String, String>::new());

    // Track how many times the work closure actually executes
    let execution_count = Arc::new(AtomicUsize::new(0));

    println!("Starting 5 concurrent requests for user:123...\n");

    // Simulate 5 concurrent requests for the same user data
    let mut handles = Vec::new();
    for i in 1..=5 {
        let group = Arc::clone(&cache_group);
        let counter = Arc::clone(&execution_count);
        let handle = tokio::spawn(async move {
            let start = tokio::time::Instant::now();

            let result = group
                .work("user:123".to_string(), || async {
                    let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    println!("  [Request {i}] I'm the leader! Fetching from database... (execution #{count})");

                    // Simulate expensive database query
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    "UserData(name: Alice, age: 30)".to_string()
                })
                .await;

            let elapsed = start.elapsed();
            println!("  [Request {i}] Got result in {elapsed:?}: {result}");
        });

        handles.push(handle);

        // Stagger the requests slightly to see the deduplication in action
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Wait for all requests to complete
    for handle in handles {
        handle.await.expect("Task panicked");
    }

    let total_executions = execution_count.load(Ordering::SeqCst);
    println!("\nAll requests completed! Database query executed {total_executions} time(s) for 5 requests.");
}
