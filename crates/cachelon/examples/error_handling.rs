// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates typed error handling with cache operations.
//!
//! Shows how to:
//! - Wrap typed errors with `Error::from_source()`
//! - Attach recovery information with `with_recovery()`
//! - Extract and handle typed errors from cache operations

use std::io::{self, ErrorKind};

use cachelon::{Cache, CacheEntry, CacheTier, Error};
use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
use tick::Clock;

/// A cache tier that fails with a typed IO error and recovery info.
#[derive(Clone)]
struct FailingCache;

impl CacheTier<String, i32> for FailingCache {
    async fn get(&self, _key: &String) -> Result<Option<CacheEntry<i32>>, Error> {
        // Wrap the IO error and attach recovery information
        Err(Error::from_source(io::Error::new(
            ErrorKind::TimedOut,
            "connection timed out",
        ))
        .with_recovery(RecoveryInfo::retry()))
    }

    async fn insert(&self, _key: &String, _entry: CacheEntry<i32>) -> Result<(), Error> {
        Ok(())
    }

    async fn invalidate(&self, _key: &String) -> Result<(), Error> {
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let cache = Cache::builder(clock).storage(FailingCache).build();

    match cache.get(&"key".to_string()).await {
        Ok(Some(entry)) => println!("Got: {}", entry.value()),
        Ok(None) => println!("Got: not found"),
        Err(e) => {
            // Check recovery info to decide how to handle
            match e.recovery().kind() {
                RecoveryKind::Retry => println!("Error is transient - retrying may help"),
                RecoveryKind::Never => println!("Error is permanent - don't retry"),
                _ => println!("Unknown recovery strategy"),
            }

            // Extract the original IO error for detailed handling
            if let Some(io_err) = e.source_as::<io::Error>() {
                println!("Underlying cause: {} ({})", io_err, io_err.kind());
            }
        }
    }
}
