// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! SymCrypt-backed value encryption example.
//!
//! Demonstrates the `symcrypt` feature's built-in `Aes256GcmCipher` via the
//! `.encrypt(&key)` convenience method. Values are encrypted with FIPS-certifiable
//! AES-256-GCM before they reach an untrusted fallback tier, while keys stay
//! plaintext so they remain usable for lookups.
//!
//! Run with:
//!
//! ```text
//! cargo run -p cachet --example encrypt_symcrypt --features "memory,serialize,symcrypt"
//! ```
//!
//! Requires the `SymCrypt` library to be available at build and run time (see the
//! crate-level `symcrypt` feature docs).

use bytesbuf::BytesView;
use cachet::{Cache, CacheEntry, CacheTier, InMemoryCache};
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();

    // In production, load the 32-byte key from a secret store — never hard-code it.
    let key = [0x42u8; 32];

    // L2: an untrusted byte-oriented tier (imagine a shared/remote store). We keep a
    // direct handle so we can peek at exactly what gets persisted there.
    let l2 = InMemoryCache::<BytesView, BytesView>::new();

    // L1 typed cache: serialize typed values to bytes, then encrypt them with the
    // built-in SymCrypt AES-256-GCM cipher before they cross into L2.
    let cache = Cache::builder::<String, String>(clock.clone())
        .memory()
        .serialize()
        .encrypt(&key)
        .fallback(Cache::builder::<BytesView, BytesView>(clock).storage(l2.clone()))
        .build();

    let key_name = "session-token".to_string();
    let secret = "super-secret-value".to_string();

    cache
        .insert(key_name.clone(), CacheEntry::new(secret.clone()))
        .await
        .expect("insert failed");

    // Peek into the untrusted L2 tier: the stored key is the plaintext (serialized)
    // key, but the value is opaque ciphertext — a leak of L2 reveals nothing.
    let stored_key = BytesView::from(serialized(&key_name));
    let stored = l2
        .get(&stored_key)
        .await
        .expect("l2 get failed")
        .expect("value should be present in L2");
    let ciphertext = stored.value().to_vec();
    println!("stored in L2 ({} bytes): {ciphertext:02x?}", ciphertext.len());
    assert_ne!(ciphertext, serialized(&secret), "L2 must hold ciphertext, not plaintext");

    // Reading back through the cache transparently decrypts and deserializes.
    let value = cache.get(&key_name).await.expect("get failed").expect("entry not found");
    println!("get({key_name}): {:?}", value.value());
    assert_eq!(*value.value(), secret);

    // A value that fails authentication — here, a ciphertext relocated to a different
    // key in the untrusted tier — is treated as a cache miss rather than leaking the
    // original plaintext, because each value is cryptographically bound to its key.
    let planted_key = BytesView::from(serialized("other-key"));
    l2.insert(planted_key, stored).await.expect("plant failed");
    let miss = cache.get(&"other-key".to_string()).await.expect("get failed");
    println!("get(other-key) after relocating ciphertext: {miss:?} (miss — value is bound to its key)");
    assert!(miss.is_none());
}

/// Reproduces the serialized (version byte + postcard) form the `.serialize()`
/// boundary produces, purely so this example can locate the stored key in L2.
fn serialized(value: &str) -> Vec<u8> {
    let mut out = vec![1u8]; // FORMAT_VERSION
    out.extend_from_slice(&postcard::to_allocvec(&value.to_string()).expect("postcard serialization failed"));
    out
}
