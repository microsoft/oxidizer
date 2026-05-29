// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates accumulating cachet telemetry into a single summary
//! per cache operation, correlated by `request_id`.
//!
//! This pattern mirrors how a TVS-style consumer would collect tier
//! outcomes, latencies, and flags into one log row per request.
//!
//! Uses `DashMap` for lock-free concurrent accumulation — safe across
//! all async runtimes, including work-stealing (tokio) and thread-per-core
//! (oxidizer), even if a task migrates between cores mid-operation.
//!
//! Run with: `cargo run --example telemetry_accumulator --features "memory,logs"`

use std::time::Duration;

use cachet::telemetry::handler::{CacheEventHandler, CacheOperationEvent, CacheTierEvent, RequestId};
use cachet::{Cache, CacheEntry};
use dashmap::DashMap;
use tick::Clock;

// ---------------------------------------------------------------------------
// Accumulated state — one entry per in-flight operation, keyed by request_id
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TierRecord {
    tier_name: String,
    outcome: String,
    duration_us: u64,
    fallback: bool,
}

/// Handler that accumulates tier events per `request_id` and prints a
/// one-line summary when the operation completes.
///
/// `DashMap` shards the map internally so concurrent operations on
/// different cores rarely contend.
struct AccumulatingHandler {
    pending: DashMap<RequestId, Vec<TierRecord>>,
}

impl AccumulatingHandler {
    fn new() -> Self {
        Self { pending: DashMap::new() }
    }
}

impl CacheEventHandler for AccumulatingHandler {
    fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
        // Eviction events have request_id > 0 when triggered synchronously
        // during an insert (capacity overflow). Background maintenance
        // evictions have request_id == 0.
        self.pending.entry(event.request_id).or_default().push(TierRecord {
            tier_name: event.tier_name.to_owned(),
            outcome: event.outcome.to_owned(),
            duration_us: u64::try_from(event.duration.as_micros()).unwrap_or(u64::MAX),
            fallback: event.fallback,
        });
    }

    fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
        let tiers = self.pending.remove(&event.request_id).map(|(_, v)| v).unwrap_or_default();

        // --- Build the summary line ---
        // A TVS consumer would pack these into a bitfield here.

        let mut flags = Vec::new();
        if event.coalesced {
            flags.push("COALESCED");
        }
        if tiers.iter().any(|t| t.fallback) {
            flags.push("FALLBACK");
        }
        let flags_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };

        // Final outcome = last tier's outcome
        let outcome = tiers.last().map_or("?", |t| t.outcome.as_str());

        print!(
            "[{}] {} -> {} ({}us total){flags_str}",
            event.cache_name,
            event.operation,
            outcome,
            event.duration.as_micros(),
        );

        // Per-tier breakdown for multi-tier caches
        if tiers.len() > 1 {
            print!(" | ");
            for (i, tier) in tiers.iter().enumerate() {
                if i > 0 {
                    print!(", ");
                }
                print!("{}={} ({}us)", tier.tier_name, tier.outcome, tier.duration_us);
            }
        }

        println!();
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Single-tier cache
    println!("=== Single-tier cache ===");
    let cache: Cache<String, String> = Cache::builder(clock.clone())
        .memory()
        .name("single")
        .event_handler(AccumulatingHandler::new())
        .build();

    cache
        .insert("key".to_string(), CacheEntry::new("value".to_string()))
        .await
        .expect("insert should succeed");
    let _ = cache.get(&"key".to_string()).await;
    let _ = cache.get(&"missing".to_string()).await;

    // Two-tier cache with fallback
    println!("\n=== Two-tier cache (L1 -> L2) ===");
    let l2 = Cache::builder::<String, String>(clock.clone()).memory().name("l2");
    let cache2: Cache<String, String> = Cache::builder(clock)
        .memory()
        .name("l1")
        .ttl(Duration::from_secs(30))
        .event_handler(AccumulatingHandler::new())
        .fallback(l2)
        .build();

    cache2
        .insert("user:1".to_string(), CacheEntry::new("Alice".to_string()))
        .await
        .expect("insert should succeed");
    let _ = cache2.get(&"user:1".to_string()).await;
    let _ = cache2.get(&"nobody".to_string()).await;

    // Capacity-limited cache — evictions correlated with inserts
    println!("\n=== Capacity-limited cache (max 2 entries) ===");
    let cache3: Cache<String, String> = Cache::builder(Clock::new_tokio())
        .memory_with(|b| b.max_capacity(2).with_eviction_telemetry())
        .name("tiny")
        .event_handler(AccumulatingHandler::new())
        .build();

    // Fill to capacity
    cache3
        .insert("a".to_string(), CacheEntry::new("1".to_string()))
        .await
        .expect("insert should succeed");
    cache3
        .insert("b".to_string(), CacheEntry::new("2".to_string()))
        .await
        .expect("insert should succeed");
    // This insert may trigger an eviction — the eviction event will carry
    // the same request_id as the insert, so the accumulator sees both the
    // insert and the eviction in one summary.
    cache3
        .insert("c".to_string(), CacheEntry::new("3".to_string()))
        .await
        .expect("insert should succeed");
}
