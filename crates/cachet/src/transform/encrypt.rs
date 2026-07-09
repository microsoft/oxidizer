// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authenticated encryption of cache values stored in an untrusted tier.
//!
//! [`AeadCipher`] is the pluggable encryption contract; [`Aes256GcmCipher`] is
//! the built-in AES-256-GCM implementation. [`EncryptedTier`] installs a cipher
//! at the storage boundary, where both the key and value are available, and
//! authenticates each value against its storage key.

use std::borrow::Cow;

use aes_gcm::aead::{Aead, AeadInPlace, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use bytesbuf::BytesView;

use crate::cache::CacheName;
use crate::telemetry::CacheTelemetry;
use crate::transform::DecodeOutcome;
use crate::{CacheEntry, CacheTier, Error, SizeError};

/// Length of the AES-GCM nonce, in bytes. Stored in front of every ciphertext.
const NONCE_SIZE: usize = 12;

/// Length of the AES-GCM authentication tag, in bytes. Stored after the ciphertext.
const TAG_SIZE: usize = 16;

/// Returns a contiguous byte slice from a [`BytesView`]. Borrows for single-span
/// views (the common case) and gathers into a `Vec` only for multi-span views.
fn to_contiguous(view: &BytesView) -> Cow<'_, [u8]> {
    let first = view.first_slice();
    if first.len() == view.len() {
        Cow::Borrowed(first)
    } else {
        let mut buf = Vec::with_capacity(view.len());
        for (slice, _) in view.slices() {
            buf.extend_from_slice(slice);
        }
        Cow::Owned(buf)
    }
}

/// Authenticated encryption with associated data (AEAD) for cache values.
///
/// Implementations turn a value's plaintext bytes into stored bytes and back,
/// authenticating a caller-supplied *associated data* (AAD) value. [`EncryptedTier`]
/// passes the entry's storage key as AAD.
///
/// # Security contract
///
/// Implementors **must** authenticate `aad`: [`decrypt`](Self::decrypt) must
/// return [`DecodeOutcome::SoftFailure`] when the `aad` does not match the value
/// supplied to [`encrypt`](Self::encrypt). This is what binds each value to its
/// storage key, preventing a value from being relocated to a different key in the
/// backing store. Implementors are also responsible for nonce discipline — use a
/// fresh nonce per [`encrypt`](Self::encrypt), or a nonce-misuse-resistant scheme.
///
/// `decrypt` distinguishes two failure modes:
/// - `Ok(DecodeOutcome::SoftFailure(_))` — the ciphertext is undecodable
///   (corrupt, truncated, tampered, wrong key, or AAD mismatch); the cache treats
///   it as a miss.
/// - `Err(_)` — the operation could not be attempted (e.g. an unavailable backend);
///   the error propagates to the caller.
pub(crate) trait AeadCipher: Send + Sync {
    /// Encrypts `plaintext`, authenticating `aad`, and returns the stored representation.
    ///
    /// # Errors
    ///
    /// Returns an error if encryption cannot be performed.
    fn encrypt(&self, aad: &[u8], plaintext: &BytesView) -> Result<BytesView, Error>;

    /// Decrypts `ciphertext`, verifying `aad`.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if decryption could not be attempted. An authentication
    /// or format failure is reported as `Ok(DecodeOutcome::SoftFailure(_))`.
    fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error>;
}

/// An AES-256-GCM implementation of [`AeadCipher`].
///
/// Encryption writes a fresh random 12-byte nonce in front of the ciphertext
/// (`nonce || ciphertext || tag`) and authenticates the AAD supplied by
/// [`EncryptedTier`] (the storage key). Decryption failures — truncation,
/// corruption, tag mismatch, AAD mismatch, or the wrong key — are reported as
/// [`DecodeOutcome::SoftFailure`], so an undecodable entry is treated as a cache
/// miss rather than a hard error.
///
/// Because each encryption uses a fresh random nonce, output is non-deterministic;
/// this cipher is applied to cache *values* only, never to keys.
///
/// # Nonce reuse
///
/// A 96-bit random nonce is safe for a very large volume of writes under one key,
/// but not unbounded: the reuse probability follows the birthday bound and becomes
/// non-negligible only after an extremely large number of writes under the same key.
/// For extreme write volumes, rotate the key periodically.
#[derive(Clone)]
pub(crate) struct Aes256GcmCipher {
    cipher: Aes256Gcm,
}

impl Aes256GcmCipher {
    /// Creates a new AES-256-GCM cipher from a 32-byte key.
    #[must_use]
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.into()),
        }
    }
}

impl std::fmt::Debug for Aes256GcmCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key material.
        f.debug_struct("Aes256GcmCipher").finish_non_exhaustive()
    }
}

impl AeadCipher for Aes256GcmCipher {
    fn encrypt(&self, aad: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        getrandom::fill(&mut nonce_bytes).map_err(|e| Error::from_message(format!("failed to generate nonce: {e}")))?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Assemble `nonce || plaintext` in one buffer, encrypt the plaintext region
        // in place, then append the detached tag. The plaintext spans are copied
        // exactly once, straight into their final location.
        //
        // This copy cannot be eliminated: AES-GCM encrypts in place and so needs a
        // single mutable, contiguous buffer, but `plaintext` is a shared, immutable
        // `BytesView` (its memory may back other views) that can also be split across
        // multiple spans. We therefore must gather it into our own writable buffer —
        // which we do directly into `result` so it is the only copy.
        let mut result = Vec::with_capacity(NONCE_SIZE + plaintext.len() + TAG_SIZE);
        result.extend_from_slice(&nonce_bytes);
        for (slice, _) in plaintext.slices() {
            result.extend_from_slice(slice);
        }

        let tag = self
            .cipher
            .encrypt_in_place_detached(nonce, aad, &mut result[NONCE_SIZE..])
            .map_err(|e| Error::from_message(format!("AES-GCM encryption failed: {e}")))?;
        result.extend_from_slice(tag.as_slice());
        Ok(result.into())
    }

    fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
        // Borrow the ciphertext contiguously (zero-copy for the common single-span
        // case; gather only when it is split across spans). The plaintext is then
        // produced by the allocating `decrypt`, which is the single unavoidable copy:
        // decryption must write plaintext into fresh memory since the input view is
        // shared and immutable and cannot be decrypted in place.
        let bytes = to_contiguous(ciphertext);
        if bytes.len() < NONCE_SIZE {
            return Ok(DecodeOutcome::SoftFailure("AES-GCM ciphertext too short: missing nonce"));
        }

        let (nonce_bytes, body) = bytes.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);

        match self.cipher.decrypt(nonce, Payload { msg: body, aad }) {
            Ok(plaintext) => Ok(DecodeOutcome::Value(plaintext.into())),
            Err(_) => Ok(DecodeOutcome::SoftFailure("AES-GCM decryption failed")),
        }
    }
}

/// A cache tier that transparently encrypts values with an [`AeadCipher`].
///
/// It wraps an inner `CacheTier<BytesView, BytesView>` (typically a remote tier
/// holding serialized bytes). On insert it encrypts the value, authenticating the
/// storage key as AAD; on get it decrypts, and an authentication failure — corrupt
/// bytes, a tampered entry, or a value relocated from a different key — surfaces as
/// a cache miss (`Ok(None)`) rather than an error. Each such failure emits a
/// `cache.decrypt_failed` telemetry event so that tampering with the backing store
/// is observable rather than silent.
pub(crate) struct EncryptedTier<S> {
    inner: S,
    cipher: Box<dyn AeadCipher>,
    telemetry: CacheTelemetry,
    name: CacheName,
}

impl<S> EncryptedTier<S> {
    pub(crate) fn new(inner: S, cipher: Box<dyn AeadCipher>, telemetry: CacheTelemetry, name: CacheName) -> Self {
        Self {
            inner,
            cipher,
            telemetry,
            name,
        }
    }
}

impl<S: std::fmt::Debug> std::fmt::Debug for EncryptedTier<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedTier").field("inner", &self.inner).finish_non_exhaustive()
    }
}

impl<S> CacheTier<BytesView, BytesView> for EncryptedTier<S>
where
    S: CacheTier<BytesView, BytesView> + Send + Sync,
{
    async fn get(&self, key: &BytesView) -> Result<Option<CacheEntry<BytesView>>, Error> {
        let Some(entry) = self.inner.get(key).await? else {
            return Ok(None);
        };
        let ttl = entry.ttl();
        let cached_at = entry.cached_at();
        let value = entry.into_value();
        // The storage key is authenticated as AAD, so a value planted under the
        // wrong key fails decryption and is treated as a miss.
        let aad = to_contiguous(key);
        match self.cipher.decrypt(aad.as_ref(), &value)? {
            DecodeOutcome::Value(value) => {
                let mut decrypted = CacheEntry::new(value);
                if let Some(ttl) = ttl {
                    decrypted.set_ttl(ttl);
                }
                if let Some(cached_at) = cached_at {
                    decrypted.ensure_cached_at(cached_at);
                }
                Ok(Some(decrypted))
            }
            DecodeOutcome::SoftFailure(_) => {
                self.telemetry.record_decrypt_failure(self.name);
                Ok(None)
            }
        }
    }

    async fn insert(&self, key: BytesView, entry: CacheEntry<BytesView>) -> Result<(), Error> {
        let aad = to_contiguous(&key);
        let encrypted = entry.try_map_value(|value| self.cipher.encrypt(aad.as_ref(), &value))?;
        self.inner.insert(key, encrypted).await
    }

    async fn invalidate(&self, key: &BytesView) -> Result<(), Error> {
        self.inner.invalidate(key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.clear().await
    }

    async fn len(&self) -> Result<u64, SizeError> {
        self.inner.len().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [42u8; 32];
    const AAD: &[u8] = b"cache-key";

    fn view(data: &[u8]) -> BytesView {
        BytesView::from(data.to_vec())
    }

    fn test_tier<S>(inner: S) -> EncryptedTier<S> {
        EncryptedTier::new(inner, Box::new(Aes256GcmCipher::new(&KEY)), CacheTelemetry::new(), "encrypted-test")
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let plaintext = view(b"the quick brown fox");

        let encrypted = cipher.encrypt(AAD, &plaintext).expect("encrypt should succeed");
        assert_ne!(encrypted.to_vec(), plaintext.to_vec(), "ciphertext must differ from plaintext");

        match cipher.decrypt(AAD, &encrypted).expect("decrypt should not hard-error") {
            DecodeOutcome::Value(v) => assert_eq!(v.to_vec(), plaintext.to_vec()),
            DecodeOutcome::SoftFailure(reason) => panic!("expected a decoded value, got soft failure: {reason}"),
        }
    }

    #[test]
    fn decrypt_with_wrong_aad_is_soft_failure() {
        // The core relocation defense: a ciphertext authenticated under one key's
        // AAD must not decrypt under a different key's AAD.
        let cipher = Aes256GcmCipher::new(&KEY);
        let encrypted = cipher.encrypt(b"key-A", &view(b"secret")).expect("encrypt should succeed");

        let outcome = cipher.decrypt(b"key-B", &encrypted).expect("decrypt should not hard-error");
        assert!(
            matches!(outcome, DecodeOutcome::SoftFailure(_)),
            "AAD mismatch must be a soft failure"
        );
    }

    #[test]
    fn each_encrypt_uses_a_fresh_nonce() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let plaintext = view(b"same input");

        let a = cipher.encrypt(AAD, &plaintext).expect("encrypt should succeed").to_vec();
        let b = cipher.encrypt(AAD, &plaintext).expect("encrypt should succeed").to_vec();
        assert_ne!(a, b, "distinct nonces should yield distinct ciphertexts for identical input");
    }

    #[test]
    fn decrypt_too_short_is_soft_failure() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let outcome = cipher
            .decrypt(AAD, &view(&[0u8; NONCE_SIZE - 1]))
            .expect("decrypt should not hard-error");
        assert!(
            matches!(outcome, DecodeOutcome::SoftFailure(_)),
            "truncated input should be a soft failure"
        );
    }

    #[test]
    fn decrypt_tampered_ciphertext_is_soft_failure() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let mut encrypted = cipher.encrypt(AAD, &view(b"secret")).expect("encrypt should succeed").to_vec();
        *encrypted.last_mut().expect("ciphertext is non-empty") ^= 0x01;

        let outcome = cipher
            .decrypt(AAD, &BytesView::from(encrypted))
            .expect("decrypt should not hard-error");
        assert!(
            matches!(outcome, DecodeOutcome::SoftFailure(_)),
            "tampered ciphertext should be a soft failure"
        );
    }

    #[test]
    fn decrypt_with_wrong_key_is_soft_failure() {
        let encrypted = Aes256GcmCipher::new(&KEY)
            .encrypt(AAD, &view(b"secret"))
            .expect("encrypt should succeed");
        let other = Aes256GcmCipher::new(&[7u8; 32]);

        let outcome = other.decrypt(AAD, &encrypted).expect("decrypt should not hard-error");
        assert!(
            matches!(outcome, DecodeOutcome::SoftFailure(_)),
            "wrong key should be a soft failure"
        );
    }

    #[test]
    fn round_trip_over_multi_span_view() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let encrypted = cipher.encrypt(AAD, &view(b"multi span payload")).expect("encrypt should succeed");

        // Split the ciphertext into two spans so decrypt must handle a multi-span view.
        let bytes = encrypted.to_vec();
        let mid = bytes.len() / 2;
        let mut multi = BytesView::from(bytes[..mid].to_vec());
        multi.append(BytesView::from(bytes[mid..].to_vec()));
        assert_ne!(multi.first_slice().len(), multi.len(), "test fixture should be multi-span");

        let outcome = cipher.decrypt(AAD, &multi).expect("decrypt should succeed");
        assert!(matches!(outcome, DecodeOutcome::Value(v) if v.to_vec() == b"multi span payload"));
    }

    #[test]
    fn round_trip_over_multi_span_plaintext() {
        // Exercise the multi-span gather path in `encrypt`: the plaintext view is
        // split across two spans, so `encrypt` must collect them into one buffer.
        let cipher = Aes256GcmCipher::new(&KEY);
        let mut plaintext = BytesView::from(b"multi span ".to_vec());
        plaintext.append(BytesView::from(b"plaintext value".to_vec()));
        assert_ne!(plaintext.first_slice().len(), plaintext.len(), "test fixture should be multi-span");

        let encrypted = cipher.encrypt(AAD, &plaintext).expect("encrypt should succeed");
        let outcome = cipher.decrypt(AAD, &encrypted).expect("decrypt should succeed");
        assert!(matches!(outcome, DecodeOutcome::Value(v) if v.to_vec() == b"multi span plaintext value"));
    }

    #[test]
    fn debug_does_not_leak_key() {
        let rendered = format!("{:?}", Aes256GcmCipher::new(&KEY));
        assert!(rendered.contains("Aes256GcmCipher"));
        assert!(!rendered.contains("42"), "Debug must not render key material");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn encrypted_tier_round_trips_through_inner() {
        use cachet_tier::MockCache;

        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = test_tier(inner.clone());

        let key = view(b"user:1");
        tier.insert(key.clone(), CacheEntry::new(view(b"profile")))
            .await
            .expect("insert should succeed");

        // The inner tier stores ciphertext, not the plaintext value.
        let stored = inner.get(&key).await.expect("inner get ok").expect("entry present");
        assert_ne!(stored.value().to_vec(), b"profile", "inner tier must hold ciphertext");

        let fetched = tier.get(&key).await.expect("get ok").expect("entry present");
        assert_eq!(fetched.value().to_vec(), b"profile", "decrypted value must match original");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn encrypted_tier_relocated_value_is_a_miss() {
        use cachet_tier::MockCache;

        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = test_tier(inner.clone());

        // Legitimately store a value under key A.
        let key_a = view(b"key-A");
        tier.insert(key_a.clone(), CacheEntry::new(view(b"value-A")))
            .await
            .expect("insert should succeed");
        let blob_a = inner.get(&key_a).await.expect("inner get ok").expect("entry present").into_value();

        // Attacker relocates A's ciphertext blob under key B in the untrusted inner tier.
        let key_b = view(b"key-B");
        inner
            .insert(key_b.clone(), CacheEntry::new(blob_a))
            .await
            .expect("insert should succeed");

        // Reading B must NOT yield A's value: AAD (key) mismatch => decryption fails => miss.
        let fetched = tier.get(&key_b).await.expect("get ok");
        assert!(fetched.is_none(), "relocated ciphertext must fail AAD check and read as a miss");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn decrypt_failure_emits_telemetry() {
        use cachet_tier::MockCache;
        use testing_aids::LogCapture;

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = EncryptedTier::new(
            inner.clone(),
            Box::new(Aes256GcmCipher::new(&KEY)),
            CacheTelemetry::with_logging(),
            "encrypted-test",
        );

        // Plant a garbage "ciphertext" that cannot authenticate.
        let key = view(b"key");
        inner
            .insert(key.clone(), CacheEntry::new(view(&[0u8; 64])))
            .await
            .expect("insert should succeed");

        let fetched = tier.get(&key).await.expect("get ok");
        assert!(fetched.is_none(), "undecryptable value must read as a miss");
        capture.assert_contains(crate::telemetry::attributes::EVENT_DECRYPT_FAILED);
    }
}
