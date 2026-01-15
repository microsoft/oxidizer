// Copyright (c) Microsoft Corporation.

//! Get-or-Insert Example
//!
//! Demonstrates the common "get from cache or compute and insert" pattern
//! using `get_or_insert` and `try_get_or_insert`.

use std::time::Duration;

use cachelon::Cache;
use tick::Clock;

/// Simulates an expensive database lookup.
async fn fetch_user_from_db(user_id: &str) -> String {
    // Simulate database latency
    tokio::time::sleep(Duration::from_millis(100)).await;
    format!("User<{}>", user_id)
}

/// Simulates a fallible API call.
async fn fetch_user_from_api(user_id: &str) -> Result<String, ApiError> {
    // Simulate API latency
    tokio::time::sleep(Duration::from_millis(50)).await;

    if user_id == "error" {
        Err(ApiError::NotFound)
    } else {
        Ok(format!("ApiUser<{}>", user_id))
    }
}

#[derive(Debug)]
enum ApiError {
    NotFound,
    #[allow(dead_code, reason = "variant used implicitly via From impl")]
    CacheError(cachelon::Error),
}

impl From<cachelon::Error> for ApiError {
    fn from(e: cachelon::Error) -> Self {
        Self::CacheError(e)
    }
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(300))
        .build();

    // Pattern 1: get_or_insert (infallible)
    // Compute expensive values only on cache miss

    // First call - cache miss, fetches from DB
    let _user = cache.get_or_insert(&"user:1".to_string(), || fetch_user_from_db("1")).await;

    // Second call - cache hit, no DB call
    let _user = cache.get_or_insert(&"user:1".to_string(), || fetch_user_from_db("1")).await;

    // Different key - cache miss
    let _user = cache.get_or_insert(&"user:2".to_string(), || fetch_user_from_db("2")).await;

    // Pattern 2: try_get_or_insert (fallible)
    // Handle errors without caching failed results

    // Successful API call
    let _result: Result<_, ApiError> = cache
        .try_get_or_insert(&"api_user:1".to_string(), || fetch_user_from_api("1"))
        .await;

    // Failed API call - error propagates, nothing cached
    let _result: Result<_, ApiError> = cache
        .try_get_or_insert(&"api_user:error".to_string(), || fetch_user_from_api("error"))
        .await;

    // Verify error case wasn't cached (API called again)
    let _result: Result<_, ApiError> = cache
        .try_get_or_insert(&"api_user:error".to_string(), || fetch_user_from_api("error"))
        .await;
}
