// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the value-encryption transform via `CacheBuilder`.
//!
//! These exercise the encryption *pipeline* (builder wiring, key-as-AAD binding,
//! relocation defense, fallback chaining) using a crypto-free mock [`AeadCipher`],
//! so they run under the base `encrypt` feature with no cryptographic dependency.
//! The SymCrypt-backed `.encrypt(&key)` convenience is exercised in a separate
//! module gated on the `symcrypt` feature.

#![cfg(all(feature = "encrypt", feature = "serialize", feature = "test-util"))]

use std::sync::atomic::{AtomicU32, Ordering};

use bytesbuf::BytesView;
use cachet::{AeadCipher, Cache, CacheEntry, CacheOp, CacheTier, DecodeOutcome, Error, MockCache};
use tick::Clock;

const NONCE_SIZE: usize = 12;

/// A crypto-free [`AeadCipher`] for exercising the pipeline. The stored form is
/// `nonce(12) || aad_len(4, LE) || aad || plaintext`. A monotonic counter stands in
/// for a fresh nonce per encryption, and `decrypt` authenticates the AAD by comparing
/// it to the embedded copy — mirroring the security contract without real crypto.
#[derive(Default)]
struct MockCipher {
    counter: AtomicU32,
}

impl AeadCipher for MockCipher {
    fn encrypt(&self, aad: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
        let nonce = self.counter.fetch_add(1, Ordering::Relaxed);
        let mut out = Vec::with_capacity(NONCE_SIZE + 4 + aad.len() + plaintext.len());
        out.extend_from_slice(&[0u8; NONCE_SIZE - 4]);
        out.extend_from_slice(&nonce.to_le_bytes());
        out.extend_from_slice(&u32::try_from(aad.len()).expect("aad fits in u32").to_le_bytes());
        out.extend_from_slice(aad);
        for (slice, _) in plaintext.slices() {
            out.extend_from_slice(slice);
        }
        Ok(BytesView::from(out))
    }

    fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
        let bytes = ciphertext.to_vec();
        let Some(rest) = bytes.get(NONCE_SIZE..) else {
            return Ok(DecodeOutcome::SoftFailure("truncated"));
        };
        let Some(len_bytes) = rest.get(..4) else {
            return Ok(DecodeOutcome::SoftFailure("truncated"));
        };
        let aad_len = u32::from_le_bytes(len_bytes.try_into().expect("4 bytes")) as usize;
        let Some(stored_aad) = rest.get(4..4 + aad_len) else {
            return Ok(DecodeOutcome::SoftFailure("truncated"));
        };
        if stored_aad != aad {
            return Ok(DecodeOutcome::SoftFailure("aad mismatch"));
        }
        let plaintext = &rest[4 + aad_len..];
        Ok(DecodeOutcome::Value(BytesView::from(plaintext.to_vec())))
    }
}

/// Returns the serialized (version byte + postcard) form of a value, matching
/// what the `serialize()` boundary produces before encryption.
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
        .encrypt_with(MockCipher::default())
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

    // Values ARE encrypted: the stored bytes differ from the plaintext-serialized form.
    let plaintext = serialized(&value);
    assert_ne!(stored_value.to_vec(), plaintext, "stored value must be ciphertext, not plaintext");

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
        .encrypt_with(MockCipher::default())
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
    // `.encrypt_with()` must be reachable after `.serialize()` on a FallbackBuilder path.
    let l3 = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(MockCache::<String, String>::new())
        .fallback(Cache::builder::<String, String>(Clock::new_frozen()).storage(MockCache::<String, String>::new()))
        .serialize()
        .encrypt_with(MockCipher::default())
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
        .encrypt_with(MockCipher::default())
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
        .encrypt_with(MockCipher::default());
    assert!(format!("{builder:?}").contains("EncryptedTransformBuilder"));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn encrypt_chained_post_transform_fallbacks() {
    // Chain two post-transform fallback tiers after `.encrypt_with()`, exercising the
    // second `.fallback()` that folds the existing post tier into a FallbackBuilder.
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<BytesView, BytesView>::new();
    let l3 = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder::<String, String>(Clock::new_frozen())
        .storage(l1.clone())
        .serialize()
        .encrypt_with(MockCipher::default())
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

/// SymCrypt-backed `.encrypt(&key)` convenience, gated on the `symcrypt` feature.
#[cfg(feature = "symcrypt")]
mod symcrypt {
    use super::{NONCE_SIZE, serialized};
    use bytesbuf::BytesView;
    use cachet::{Cache, CacheOp, CacheTier, MockCache};
    use tick::Clock;

    const KEY: [u8; 32] = [42u8; 32];
    const GCM_TAG_SIZE: usize = 16;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn symcrypt_encrypt_round_trips_with_gcm_overhead() {
        let l1 = MockCache::<String, String>::new();
        let l2 = MockCache::<BytesView, BytesView>::new();

        let cache = Cache::builder::<String, String>(Clock::new_frozen())
            .storage(l1.clone())
            .serialize()
            .encrypt(&KEY)
            .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(l2.clone()))
            .build();

        let value = "Hello, world!".to_string();
        cache
            .insert("greeting".to_string(), value.clone())
            .await
            .expect("insert should succeed");

        let stored_value = l2
            .operations()
            .iter()
            .find_map(|op| match op {
                CacheOp::Insert { entry, .. } => Some(entry.value().to_vec()),
                _ => None,
            })
            .expect("post-transform tier should have received an insert");

        let plaintext = serialized(&value);
        assert_ne!(stored_value, plaintext, "stored value must be ciphertext");
        assert_eq!(
            stored_value.len(),
            NONCE_SIZE + plaintext.len() + GCM_TAG_SIZE,
            "ciphertext must be nonce + plaintext + GCM tag"
        );

        l1.invalidate(&"greeting".to_string()).await.expect("invalidate should succeed");
        let fetched = cache
            .get(&"greeting".to_string())
            .await
            .expect("get should succeed")
            .expect("value should be present");
        assert_eq!(*fetched.value(), value, "decrypted value must match the original");
    }
}
