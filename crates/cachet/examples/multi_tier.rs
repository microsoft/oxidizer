// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multi-tier cache with conditional promotion policies.
//! Example: only promote "not found" results to avoid repeated backend queries.

use std::fmt;
use std::future::ready;
use std::sync::Arc;
use std::time::Duration;

use cachet::{Cache, CacheEntry, CacheTier, Error, InsertOutcome, InsertPolicy};
use parking_lot::Mutex;
use tick::Clock;

#[derive(Clone, Debug, PartialEq)]
enum UserData {
    Found(String),
    NotFound,
}

impl fmt::Display for UserData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Found(name) => write!(f, "Found({name})"),
            Self::NotFound => write!(f, "NotFound"),
        }
    }
}

#[derive(Debug)]
struct Database(Mutex<u32>);

impl CacheTier<String, UserData> for Arc<Database> {
    fn get(&self, key: &String) -> impl Future<Output = Result<Option<CacheEntry<UserData>>, Error>> + Send {
        *self.0.lock() += 1;
        let data = match key.as_str() {
            "user:1" => UserData::Found("Alice".to_string()),
            _ => UserData::NotFound,
        };
        ready(Ok(Some(CacheEntry::new(data))))
    }

    fn insert(&self, _: String, _: CacheEntry<UserData>) -> impl Future<Output = Result<InsertOutcome, Error>> + Send {
        ready(Ok(InsertOutcome::Accepted))
    }

    fn invalidate(&self, _: &String) -> impl Future<Output = Result<(), Error>> + Send {
        ready(Ok(()))
    }

    fn clear(&self) -> impl Future<Output = Result<(), Error>> + Send {
        ready(Ok(()))
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let db = Arc::new(Database(Mutex::new(0)));

    // L2: database
    let l2 = Cache::builder::<String, UserData>(clock.clone()).storage(Arc::clone(&db));

    // L1: only promote NotFound (negative cache)
    let cache = Cache::builder::<String, UserData>(clock)
        .memory()
        .ttl(Duration::from_mins(1))
        .insert_policy(InsertPolicy::when(|e: &CacheEntry<UserData>| {
            matches!(e.value(), UserData::NotFound)
        }))
        .fallback(l2)
        .build();

    // user:1 exists - NOT cached (policy rejects Found)
    let v = cache.get(&"user:1".to_string()).await.expect("get failed");
    match v {
        Some(e) => println!("user:1: {}", e.value()),
        None => println!("user:1: not found"),
    }

    // user:2 not found - cached (policy accepts NotFound)
    let v = cache.get(&"user:2".to_string()).await.expect("get failed");
    match v {
        Some(e) => println!("user:2: {}", e.value()),
        None => println!("user:2: not found"),
    }

    println!("db calls after first round: {}", *db.0.lock());

    // Second round
    cache.get(&"user:1".to_string()).await.expect("get failed"); // db call (not cached)
    cache.get(&"user:2".to_string()).await.expect("get failed"); // cache hit (was promoted)

    println!("db calls after second round: {}", *db.0.lock());
}
