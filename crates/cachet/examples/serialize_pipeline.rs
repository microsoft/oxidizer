// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialize/compress/encrypt pipeline example.
//!
//! Demonstrates a two-tier cache where L1 is in-memory with typed keys/values,
//! and L2 is a remote tier (simulated with MockCache) that stores serialized,
//! compressed, and encrypted bytes.
//!
//! Data flow on insert:
//!   String,MyValue → serialize(bincode) → compress(zstd) → encrypt(aes) → L2
//!
//! Data flow on get (L1 miss):
//!   L2 → decrypt → decompress → deserialize → String,MyValue

use cachet::{AesGcmDecoder, AesGcmEncoder, BincodeDecoder, BincodeEncoder, Cache, CacheEntry, MockCache, ZstdDecoder, ZstdEncoder};
use tick::Clock;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct UserProfile {
    name: String,
    age: u32,
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // Encryption key (in production, load from a secret store)
    let key: [u8; 32] = [42; 32];

    // L2: a remote tier that stores Vec<u8> keys and Vec<u8> values.
    // In a real app, this would be Redis, S3, etc.
    let remote = Cache::builder::<Vec<u8>, Vec<u8>>(clock.clone()).storage(MockCache::new());

    // Build the cache with the full pipeline:
    //   L1 (memory) → serialize → compress → encrypt → L2 (remote)
    let cache = Cache::builder::<String, UserProfile>(clock)
        .memory()
        .serialize(BincodeEncoder, BincodeEncoder, BincodeDecoder)
        .compress(ZstdEncoder::new(3), ZstdDecoder)
        .encrypt(AesGcmEncoder::new(&key), AesGcmDecoder::new(&key))
        .fallback(remote)
        .build();

    // Insert a value — it flows through serialize → compress → encrypt → L2
    let profile = UserProfile {
        name: "Alice".to_string(),
        age: 30,
    };
    cache
        .insert("user:1".to_string(), CacheEntry::new(profile.clone()))
        .await
        .expect("insert failed");

    // Retrieve — on L1 hit, returns directly.
    let result = cache.get(&"user:1".to_string()).await.expect("get failed");
    match result {
        Some(entry) => println!("got: {:?}", entry.value()),
        None => println!("not found"),
    }

    // Invalidate L1 to force an L2 lookup on next get
    cache.invalidate(&"user:1".to_string()).await.expect("invalidate failed");

    println!("done");
}
