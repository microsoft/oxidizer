// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates subscribing to cachet telemetry events using a custom tracing Layer.
//!
//! This example shows how to use the public constants in `cachet::telemetry::attributes`
//! to build a Layer that reacts to specific cache events.

use std::time::Duration;

use cachet::telemetry::attributes;
use cachet::{Cache, CacheEntry};
use tick::Clock;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

/// A simple Layer that prints cache events to stdout.
struct CacheEventPrinter;

/// Visitor that extracts cache telemetry fields from a tracing event.
#[derive(Default)]
struct CacheFieldVisitor {
    cache_name: Option<String>,
    event: Option<String>,
    duration_ns: Option<u128>,
}

impl Visit for CacheFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            attributes::FIELD_NAME => self.cache_name = Some(value.to_owned()),
            attributes::FIELD_EVENT => self.event = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if field.name() == attributes::FIELD_DURATION_NS {
            self.duration_ns = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl<S: tracing::Subscriber> Layer<S> for CacheEventPrinter {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        // Only process cachet events
        if !event.metadata().target().starts_with(attributes::TARGET) {
            return;
        }

        let mut visitor = CacheFieldVisitor::default();
        event.record(&mut visitor);

        let cache_name = visitor.cache_name.as_deref().unwrap_or("unknown");
        let event_type = visitor.event.as_deref().unwrap_or("unknown");
        let duration_us = visitor.duration_ns.unwrap_or(0) / 1000;

        // React to specific events using the public constants
        match event_type {
            attributes::EVENT_HIT => println!("HIT  on {cache_name} ({duration_us}µs)"),
            attributes::EVENT_MISS => println!("MISS on {cache_name} ({duration_us}µs)"),
            attributes::EVENT_INSERTED => println!("INSERT on {cache_name} ({duration_us}µs)"),
            attributes::EVENT_EXPIRED => println!("EXPIRED on {cache_name} ({duration_us}µs)"),
            other => println!("{other} on {cache_name} ({duration_us}µs)"),
        }
    }
}

#[tokio::main]
async fn main() {
    // Set up the subscriber with our custom cache event layer
    let subscriber = tracing_subscriber::registry().with(CacheEventPrinter);
    let _guard = tracing::subscriber::set_default(subscriber);

    let clock = Clock::new_tokio();
    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .enable_logs()
        .ttl(Duration::from_secs(30))
        .build();

    println!("Inserting entry...");
    cache
        .insert("user:1".to_string(), CacheEntry::new("Alice".to_string()))
        .await
        .expect("insert failed");

    println!("Getting existing key...");
    let _ = cache.get(&"user:1".to_string()).await;

    println!("Getting missing key...");
    let _ = cache.get(&"user:999".to_string()).await;
}
