// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates typed error handling with custom CacheTier implementations.
//!
//! Shows how CacheTier implementers can wrap typed errors and how consumers
//! can extract and handle them.

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::sync::{Arc, Mutex};

use cachelon::{Cache, CacheEntry, CacheTier, Error};
use tick::Clock;

/// A simulated cache that can fail with IO errors.
///
/// In a real implementation, this might be a Redis client, file-based cache, etc.
#[derive(Clone, Default)]
struct SimulatedStorageCache {
    data: Arc<Mutex<HashMap<String, i32>>>,
    should_fail: Arc<Mutex<Option<ErrorKind>>>,
}

impl SimulatedStorageCache {
    fn new() -> Self {
        Self::default()
    }

    /// Simulate a failure on the next operation.
    fn simulate_failure(&self, kind: ErrorKind) {
        *self.should_fail.lock().unwrap() = Some(kind);
    }

    fn check_failure(&self) -> Result<(), Error> {
        if let Some(kind) = self.should_fail.lock().unwrap().take() {
            // Wrap the IO error using from_source to preserve the type
            return Err(Error::from_source(io::Error::new(kind, format!("simulated {kind:?} error"))));
        }
        Ok(())
    }
}

impl CacheTier<String, i32> for SimulatedStorageCache {
    async fn get(&self, key: &String) -> Result<Option<CacheEntry<i32>>, Error> {
        self.check_failure()?;
        Ok(self.data.lock().unwrap().get(key).copied().map(CacheEntry::new))
    }

    async fn insert(&self, key: &String, entry: CacheEntry<i32>) -> Result<(), Error> {
        self.check_failure()?;
        self.data.lock().unwrap().insert(key.clone(), *entry.value());
        Ok(())
    }

    async fn invalidate(&self, key: &String) -> Result<(), Error> {
        self.check_failure()?;
        self.data.lock().unwrap().remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        self.check_failure()?;
        self.data.lock().unwrap().clear();
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let storage = SimulatedStorageCache::new();
    let cache = Cache::builder(clock).storage(storage.clone()).build();

    // Normal operation
    cache
        .insert(&"key".to_string(), CacheEntry::new(42))
        .await
        .expect("insert should succeed");

    println!("Inserted key=42");

    // Simulate a connection timeout
    storage.simulate_failure(ErrorKind::TimedOut);

    match cache.get(&"key".to_string()).await {
        Ok(entry) => println!("Got: {:?}", entry.map(|e| *e.value())),
        Err(e) => {
            // Extract the original IO error and handle based on kind
            if let Some(io_err) = e.source_as::<io::Error>() {
                match io_err.kind() {
                    ErrorKind::TimedOut => {
                        println!("Operation timed out - could retry");
                    }
                    ErrorKind::ConnectionRefused => {
                        println!("Connection refused - server may be down");
                    }
                    ErrorKind::PermissionDenied => {
                        println!("Permission denied - check credentials");
                    }
                    other => {
                        println!("IO error: {other:?}");
                    }
                }
            } else {
                println!("Unknown error: {e}");
            }
        }
    }

    // Simulate permission denied
    storage.simulate_failure(ErrorKind::PermissionDenied);

    match cache.get(&"key".to_string()).await {
        Ok(_) => println!("Unexpected success"),
        Err(e) if e.is_source::<io::Error>() => {
            let io_err = e.source_as::<io::Error>().unwrap();
            println!("Caught IO error: {} (kind={:?})", io_err, io_err.kind());
        }
        Err(e) => println!("Other error: {e}"),
    }

    // Normal operation resumes
    let value = cache.get(&"key".to_string()).await.expect("get should succeed");
    println!("After recovery: {:?}", value.map(|e| *e.value()));
}
