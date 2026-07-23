// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the value-protection transform via `CacheBuilder`.
//!
//! These exercise the protection *pipeline* (builder wiring, key-as-context binding,
//! relocation defense, fallback chaining) using the crypto-free [`MockValueProtector`]
//! shipped under `test-util`, so they run with no cryptographic dependency.

#![cfg(all(feature = "encrypt", feature = "serialize", feature = "test-util"))]

// Integration binaries link the library with `cfg(test)` false, so the crate-root
// tracing initialization does not run here. Install it directly. See docs/tracing-tests.md.
testing_aids::init_tracing!();

use bytesbuf::BytesView;
use cachet::{Cache, CacheEntry, CacheOp, CacheTier, MockCache, MockValueProtector};
use tick::Clock;

/// Returns the serialized (version byte + postcard) form of a value, matching
/// what the `serialize()` boundary produces before protection.
fn serialized(value: &str) -> Vec<u8> {
    let mut out = vec![1u8]; // FORMAT_VERSION
    out.extend_from_slice(&postcard::to_allocvec(&value.to_string()).expect("postcard serialization should not fail"));
    out
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn encrypt_pipeline_stores_ciphertext_and_round_trips() {
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<BytesView, BytesView>::new();

    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(l1.clone())
        .serialize()
        .protect_with(MockValueProtector::new())
        .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(l2.clone()))
        .build();

    let key = "greeting".to_string();
    let value = "Hello, world!".to_string();
    cache.insert(key.clone(), value.clone()).await.expect("insert should succeed");

    // Inspect what actually landed in the post-transform tier.
    let after_ops = l2.operations();
    let insert = after_ops
        .iter()
        .find_map(|op| match op {
            CacheOp::Insert { key, entry } => Some((key.clone(), entry.value().clone())),
            _ => None,
        })
        .expect("post-transform tier should have received an insert");
    let (stored_key, stored_value) = insert;

    // Keys are NOT encrypted (encryption is non-deterministic), so the stored key
    // is exactly the serialized key and remains lookupable.
    assert_eq!(stored_key.to_vec(), serialized(&key), "key must be serialized but not encrypted");

    // Values ARE encrypted: the stored bytes differ from the plaintext-serialized
    // form, and the plaintext never appears verbatim anywhere in the ciphertext.
    let plaintext = serialized(&value);
    let stored = stored_value.to_vec();
    assert_ne!(stored, plaintext, "stored value must be ciphertext, not plaintext");
    assert!(
        !stored.windows(plaintext.len()).any(|w| w == plaintext.as_slice()),
        "plaintext must not appear verbatim in the stored ciphertext"
    );

    // Force the read to fall back to the encrypted tier and decrypt.
    l1.invalidate(&key).await.expect("invalidate should succeed");
    let fetched = cache.get(&key).await.expect("get should succeed").expect("value should be present");
    assert_eq!(*fetched.value(), value, "decrypted value must match the original");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn encrypt_each_insert_uses_fresh_nonce() {
    let l2 = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(MockCache::<String, String>::new())
        .serialize()
        .protect_with(MockValueProtector::new())
        .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(l2.clone()))
        .build();

    // Insert the same key/value twice; the ciphertext must differ each time.
    cache
        .insert("k".to_string(), "same".to_string())
        .await
        .expect("insert should succeed");
    cache
        .insert("k".to_string(), "same".to_string())
        .await
        .expect("insert should succeed");

    let ciphertexts: Vec<Vec<u8>> = l2
        .operations()
        .iter()
        .filter_map(|op| match op {
            CacheOp::Insert { entry, .. } => Some(entry.value().to_vec()),
            _ => None,
        })
        .collect();
    assert_eq!(ciphertexts.len(), 2, "both inserts should reach the encrypted tier");
    assert_ne!(ciphertexts[0], ciphertexts[1], "each encryption must use a fresh nonce");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn encrypt_on_fallback_builder() {
    // `.protect_with()` must be reachable after `.serialize()` on a FallbackBuilder path.
    let l3 = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(MockCache::<String, String>::new())
        .fallback(Cache::builder::<String, String>(Clock::new_frozen()).storage(MockCache::<String, String>::new()))
        .serialize()
        .protect_with(MockValueProtector::new())
        .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(l3.clone()))
        .build();

    cache
        .insert("key".to_string(), "value".to_string())
        .await
        .expect("insert should succeed");

    let l3_ops = l3.operations();
    let stored_value = l3_ops
        .iter()
        .find_map(|op| match op {
            CacheOp::Insert { entry, .. } => Some(entry.value().to_vec()),
            _ => None,
        })
        .expect("encrypted tier should have received an insert");
    assert_ne!(
        stored_value,
        serialized("value"),
        "value must be encrypted through the fallback path"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn relocated_ciphertext_reads_as_a_miss() {
    // End-to-end: a value is bound to its key via AAD, so an attacker who moves a
    // valid ciphertext blob to a different key in the untrusted remote tier cannot
    // make it decrypt — the read is a miss, not a leak of the other key's value.
    let l1 = MockCache::<String, String>::new();
    let remote = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(l1.clone())
        .serialize()
        .protect_with(MockValueProtector::new())
        .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(remote.clone()))
        .build();

    // Legitimately cache A -> "secret-A".
    cache
        .insert("A".to_string(), "secret-A".to_string())
        .await
        .expect("insert should succeed");

    // Recover A's stored key and ciphertext blob from the remote tier.
    let stored = remote
        .operations()
        .iter()
        .find_map(|op| match op {
            CacheOp::Insert { key, entry } => Some((key.clone(), entry.value().clone())),
            _ => None,
        })
        .expect("remote tier should have received an insert");
    let (stored_key_a, blob_a) = stored;
    assert_eq!(stored_key_a.to_vec(), serialized("A"), "sanity: key stored is serialized key A");

    // Attacker relocates A's ciphertext under key B in the untrusted remote tier.
    let key_b = BytesView::from(serialized("B"));
    remote
        .insert(key_b, CacheEntry::new(blob_a))
        .await
        .expect("planting the blob should succeed");

    // Reading B must fail the AAD check and read as a miss — never A's value.
    let result = cache.get(&"B".to_string()).await.expect("get should succeed");
    assert!(result.is_none(), "relocated ciphertext must not decrypt under a different key");
}

#[cfg_attr(miri, ignore)]
#[test]
fn encrypted_transform_builder_debug() {
    let builder = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(MockCache::<String, String>::new())
        .serialize()
        .protect_with(MockValueProtector::new());
    assert!(format!("{builder:?}").contains("ProtectedTransformBuilder"));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn encrypt_chained_post_transform_fallbacks() {
    // Chain two post-transform fallback tiers after `.protect_with()`, exercising the
    // second `.fallback()` that folds the existing post tier into a FallbackBuilder.
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<BytesView, BytesView>::new();
    let l3 = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(l1.clone())
        .serialize()
        .protect_with(MockValueProtector::new())
        .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(l2.clone()))
        .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(l3.clone()))
        .build();

    cache.insert("k".to_string(), "v".to_string()).await.expect("insert should succeed");

    // Force a read past L1 so the encrypted post chain decrypts the value.
    l1.invalidate(&"k".to_string()).await.expect("invalidate should succeed");
    let fetched = cache
        .get(&"k".to_string())
        .await
        .expect("get should succeed")
        .expect("value present");
    assert_eq!(
        *fetched.value(),
        "v",
        "value must round-trip through the chained encrypted fallbacks"
    );

    // The first post tier stored ciphertext, not plaintext.
    let stored = l2
        .operations()
        .iter()
        .find_map(|op| match op {
            CacheOp::Insert { entry, .. } => Some(entry.value().to_vec()),
            _ => None,
        })
        .expect("first post tier should have received an insert");
    assert_ne!(stored, serialized("v"), "value must be encrypted in the chained fallback");
}
