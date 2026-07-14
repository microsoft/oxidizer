// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authenticated encryption of cache values stored in an untrusted tier.
//!
//! The base `encrypt` feature provides only the encryption *mechanism* — it carries
//! no cryptographic dependency of its own. [`AeadCipher`] is the pluggable contract:
//! you supply the actual cipher, backed by your approved cryptographic library, and
//! register it with [`encrypt_with`](crate::TransformBuilder::encrypt_with).
//! [`EncryptedTier`] installs that cipher at the storage boundary, where both the key
//! and value are available, and authenticates each value against its storage key.
//!
//! If you don't need to supply your own cipher, enable the optional `symcrypt` feature
//! to get a ready-made, FIPS-certifiable implementation (`Aes256GcmCipher`) plus the
//! `encrypt(&key)` convenience method.

use std::borrow::Cow;

use bytesbuf::BytesView;

use crate::cache::CacheName;
use crate::telemetry::CacheTelemetry;
use crate::transform::DecodeOutcome;
use crate::{CacheEntry, CacheTier, Error, SizeError};

/// Returns a contiguous byte slice from a [`BytesView`]. Borrows for single-span
/// views (the common case) and gathers into a `Vec` only for multi-span views.
pub(crate) fn to_contiguous(view: &BytesView) -> Cow<'_, [u8]> {
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
/// authenticating a caller-supplied *associated data* (AAD) value. `EncryptedTier`
/// passes the entry's storage key as AAD, so a value is cryptographically bound to
/// the key it was stored under.
///
/// The base `encrypt` feature supplies no cipher of its own: implement this trait
/// with your organization's approved cryptographic library and register it via
/// [`encrypt_with`](crate::TransformBuilder::encrypt_with). Alternatively, enable the
/// `symcrypt` feature for the built-in `Aes256GcmCipher`.
///
/// # Security contract
///
/// Implementors **must** authenticate `aad`: [`decrypt`](Self::decrypt) must return
/// [`DecodeOutcome::SoftFailure`] when the `aad` does not match the value supplied to
/// [`encrypt`](Self::encrypt). This is what binds each value to its storage key,
/// preventing a value from being relocated to a different key in the backing store.
/// Implementors using a nonce-based scheme are responsible for nonce discipline — use
/// a fresh nonce per [`encrypt`](Self::encrypt), or a nonce-misuse-resistant scheme.
///
/// [`decrypt`](Self::decrypt) distinguishes two failure modes:
/// - `Ok(DecodeOutcome::SoftFailure(_))` — the ciphertext is undecodable (corrupt,
///   truncated, tampered, wrong key, or AAD mismatch); the cache treats it as a miss.
/// - `Err(_)` — the operation could not be attempted (e.g. an unavailable backend);
///   the error propagates to the caller.
pub trait AeadCipher: Send + Sync {
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
    /// Returns `Err` only if decryption could not be attempted. An authentication or
    /// format failure is reported as `Ok(DecodeOutcome::SoftFailure(_))`.
    fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error>;
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
    use cachet_tier::MockCache;

    use super::*;

    fn view(data: &[u8]) -> BytesView {
        BytesView::from(data.to_vec())
    }

    /// A crypto-free [`AeadCipher`] for exercising the tier mechanism. It "seals"
    /// a value as `aad_len || aad || plaintext` and, on decrypt, treats an AAD
    /// mismatch or malformed input as a soft failure — mirroring how a real AEAD
    /// binds the value to its key without performing any real cryptography.
    struct MockAeadCipher;

    impl AeadCipher for MockAeadCipher {
        fn encrypt(&self, aad: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
            let plaintext = plaintext.to_vec();
            let mut out = Vec::with_capacity(4 + aad.len() + plaintext.len());
            out.extend_from_slice(&(u32::try_from(aad.len()).expect("aad fits in u32")).to_le_bytes());
            out.extend_from_slice(aad);
            out.extend_from_slice(&plaintext);
            Ok(out.into())
        }

        fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
            let bytes = ciphertext.to_vec();
            let Some(len_bytes) = bytes.get(0..4) else {
                return Ok(DecodeOutcome::SoftFailure("mock: missing length prefix"));
            };
            let aad_len = u32::from_le_bytes(len_bytes.try_into().expect("4 bytes")) as usize;
            let Some(stored_aad) = bytes.get(4..4 + aad_len) else {
                return Ok(DecodeOutcome::SoftFailure("mock: truncated aad"));
            };
            if stored_aad != aad {
                return Ok(DecodeOutcome::SoftFailure("mock: aad mismatch"));
            }
            Ok(DecodeOutcome::Value(bytes[4 + aad_len..].to_vec().into()))
        }
    }

    /// A cipher whose operations always hard-error, for exercising error propagation.
    struct FailingCipher;

    impl AeadCipher for FailingCipher {
        fn encrypt(&self, _aad: &[u8], _plaintext: &BytesView) -> Result<BytesView, Error> {
            Err(Error::from_message("encrypt failed"))
        }

        fn decrypt(&self, _aad: &[u8], _ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
            Err(Error::from_message("decrypt failed"))
        }
    }

    fn tier<S>(inner: S) -> EncryptedTier<S> {
        EncryptedTier::new(inner, Box::new(MockAeadCipher), CacheTelemetry::new(), "encrypted-test")
    }

    fn failing_tier<S>(inner: S) -> EncryptedTier<S> {
        EncryptedTier::new(inner, Box::new(FailingCipher), CacheTelemetry::new(), "failing-test")
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn round_trips_through_inner() {
        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = tier(inner.clone());

        let key = view(b"user:1");
        tier.insert(key.clone(), CacheEntry::new(view(b"profile")))
            .await
            .expect("insert should succeed");

        // The inner tier stores the sealed representation, not the plaintext value.
        let stored = inner.get(&key).await.expect("inner get ok").expect("entry present");
        assert_ne!(stored.value().to_vec(), b"profile", "inner tier must not hold plaintext");

        let fetched = tier.get(&key).await.expect("get ok").expect("entry present");
        assert_eq!(fetched.value().to_vec(), b"profile", "decrypted value must match original");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn relocated_value_is_a_miss() {
        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = tier(inner.clone());

        let key_a = view(b"key-A");
        tier.insert(key_a.clone(), CacheEntry::new(view(b"value-A")))
            .await
            .expect("insert should succeed");
        let blob_a = inner.get(&key_a).await.expect("inner get ok").expect("entry present").into_value();

        // Attacker relocates A's sealed blob under key B in the untrusted inner tier.
        let key_b = view(b"key-B");
        inner
            .insert(key_b.clone(), CacheEntry::new(blob_a))
            .await
            .expect("insert should succeed");

        // Reading B must NOT yield A's value: AAD (key) mismatch => miss.
        assert!(
            tier.get(&key_b).await.expect("get ok").is_none(),
            "relocated value must fail the AAD check and read as a miss"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_miss_returns_none() {
        let tier = tier(MockCache::<BytesView, BytesView>::new());
        assert!(
            tier.get(&view(b"absent")).await.expect("get ok").is_none(),
            "empty inner tier must miss"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn preserves_ttl_and_cached_at() {
        use std::time::{Duration, SystemTime};

        let tier = tier(MockCache::<BytesView, BytesView>::new());
        let ttl = Duration::from_mins(5);
        let cached_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let key = view(b"k");

        tier.insert(key.clone(), CacheEntry::expires_at(view(b"v"), ttl, cached_at))
            .await
            .expect("insert should succeed");

        let fetched = tier.get(&key).await.expect("get ok").expect("entry present");
        assert_eq!(fetched.value().to_vec(), b"v", "decrypted value must match");
        assert_eq!(fetched.ttl(), Some(ttl), "ttl must survive the round trip");
        assert_eq!(fetched.cached_at(), Some(cached_at), "cached_at must survive the round trip");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn invalidate_clear_len_delegate() {
        let tier = tier(MockCache::<BytesView, BytesView>::new());
        tier.insert(view(b"a"), CacheEntry::new(view(b"1")))
            .await
            .expect("insert should succeed");
        tier.insert(view(b"b"), CacheEntry::new(view(b"2")))
            .await
            .expect("insert should succeed");
        assert_eq!(tier.len().await.expect("len ok"), 2);

        tier.invalidate(&view(b"a")).await.expect("invalidate should succeed");
        assert_eq!(tier.len().await.expect("len ok"), 1);

        tier.clear().await.expect("clear should succeed");
        assert_eq!(tier.len().await.expect("len ok"), 0);
    }

    #[test]
    fn debug_omits_inner_secrets() {
        let tier = tier(MockCache::<BytesView, BytesView>::new());
        assert!(format!("{tier:?}").contains("EncryptedTier"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn decrypt_failure_emits_telemetry() {
        use testing_aids::LogCapture;

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = EncryptedTier::new(
            inner.clone(),
            Box::new(MockAeadCipher),
            CacheTelemetry::with_logging(),
            "encrypted-test",
        );

        // Plant a malformed blob that cannot be decoded.
        inner
            .insert(view(b"k"), CacheEntry::new(view(&[0u8; 2])))
            .await
            .expect("insert should succeed");

        assert!(
            tier.get(&view(b"k")).await.expect("get ok").is_none(),
            "undecodable value must read as a miss"
        );
        capture.assert_contains(crate::telemetry::attributes::EVENT_DECRYPT_FAILED);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn encrypt_error_propagates_on_insert() {
        let tier = failing_tier(MockCache::<BytesView, BytesView>::new());
        assert!(
            tier.insert(view(b"k"), CacheEntry::new(view(b"v"))).await.is_err(),
            "a cipher encryption error must propagate from insert"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn decrypt_error_propagates_on_get() {
        let inner = MockCache::<BytesView, BytesView>::new();
        inner
            .insert(view(b"k"), CacheEntry::new(view(b"stored")))
            .await
            .expect("insert should succeed");

        let tier = failing_tier(inner);
        assert!(
            tier.get(&view(b"k")).await.is_err(),
            "a cipher decryption hard error must propagate from get"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn inner_tier_errors_propagate() {
        use cachet_tier::CacheOp;

        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = tier(inner.clone());

        inner.fail_when(|_op: &CacheOp<BytesView, BytesView>| true);

        assert!(tier.get(&view(b"k")).await.is_err(), "inner get error must propagate");
        assert!(
            tier.insert(view(b"k"), CacheEntry::new(view(b"v"))).await.is_err(),
            "inner insert error must propagate"
        );
        assert!(tier.invalidate(&view(b"k")).await.is_err(), "inner invalidate error must propagate");
        assert!(tier.clear().await.is_err(), "inner clear error must propagate");
    }
}
