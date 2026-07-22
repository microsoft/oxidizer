// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authenticated protection of cache values stored in an untrusted tier.
//!
//! This provides only the protection *mechanism* — it carries no cryptographic
//! dependency of its own. [`ValueProtector`] is the pluggable contract: you supply the
//! actual implementation, backed by your approved cryptographic library, and register
//! it with [`protect_with`](crate::TransformBuilder::protect_with). [`ProtectedTier`]
//! installs that protector at the storage boundary, where both the key and value are
//! available, and binds each value to its storage key.
//!
//! See the crate-level "Encryption Boundary" docs for a reference `ValueProtector`
//! implementation backed by `SymCrypt` (FIPS-certifiable AES-256-GCM).

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

/// Authenticated protection of cache values before they reach an untrusted tier.
///
/// Implementations turn a value's plaintext bytes into stored bytes and back, binding
/// a caller-supplied *context* value. `ProtectedTier` passes the entry's storage key as
/// the context, so a value is cryptographically bound to the key it was stored under.
///
/// This trait supplies no implementation of its own: implement it with your
/// organization's approved cryptographic library and register it via
/// [`protect_with`](crate::TransformBuilder::protect_with). See the crate-level
/// "Encryption Boundary" docs for a reference `SymCrypt`-backed implementation.
///
/// # Security contract
///
/// Implementors **must** bind `context`: [`unprotect`](Self::unprotect) must return
/// [`DecodeOutcome::SoftFailure`] when the `context` does not match the value supplied
/// to [`protect`](Self::protect). This is what binds each value to its storage key,
/// preventing a value from being relocated to a different key in the backing store.
/// Implementors using a nonce-based scheme are responsible for nonce discipline — use
/// a fresh nonce per [`protect`](Self::protect), or a nonce-misuse-resistant scheme.
///
/// [`unprotect`](Self::unprotect) distinguishes two failure modes:
/// - `Ok(DecodeOutcome::SoftFailure(_))` — the stored value is unrecoverable (corrupt,
///   truncated, tampered, wrong key, or context mismatch); the cache treats it as a
///   miss.
/// - `Err(_)` — the operation could not be attempted (e.g. an unavailable backend);
///   the error propagates to the caller.
pub trait ValueProtector: Send + Sync {
    /// Protects `plaintext`, binding `context`, and returns the stored representation.
    ///
    /// # Errors
    ///
    /// Returns an error if protection cannot be performed.
    fn protect(&self, context: &[u8], plaintext: &BytesView) -> Result<BytesView, Error>;

    /// Recovers a value previously protected under `context`.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if the operation could not be attempted. An authentication or
    /// format failure is reported as `Ok(DecodeOutcome::SoftFailure(_))`.
    fn unprotect(&self, context: &[u8], protected: &BytesView) -> Result<DecodeOutcome<BytesView>, Error>;
}

/// Length of the mock protector's nonce prefix, in bytes.
#[cfg(any(feature = "test-util", test))]
const MOCK_NONCE_SIZE: usize = 12;

/// A deterministic, crypto-free [`ValueProtector`] for tests.
///
/// Available with the `test-util` feature. Use it to exercise a
/// [`protect_with`](crate::TransformBuilder::protect_with) pipeline — round-trips, key
/// binding, and unprotect failures — without a real cryptographic library or a source
/// of entropy, keeping tests fast and reproducible.
///
/// The stored form is `nonce || context_len || context || masked_body`. The nonce comes
/// from a monotonic counter (so repeated `protect` calls of identical input still
/// differ, yet stay reproducible), and `masked_body` is the plaintext combined with a
/// nonce-derived keystream via XOR. It binds the `context`: [`unprotect`](ValueProtector::unprotect)
/// returns [`DecodeOutcome::SoftFailure`] on a context mismatch, truncation, or
/// corruption, mirroring the [`ValueProtector`] security contract.
///
/// # Security
///
/// This provides **no confidentiality or integrity** — the transform is trivially
/// reversible and the key is ignored. It is gated behind `test-util` and must never be
/// used in production.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "serialize", feature = "memory"))] {
/// use cachet::{Cache, MockValueProtector};
/// use tick::Clock;
///
/// let clock = Clock::new_frozen();
/// let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
/// let cache = Cache::builder::<String, String>(clock)
///     .memory()
///     .serialize()
///     .protect_with(MockValueProtector::new())
///     .fallback(remote)
///     .build();
/// # }
/// ```
#[cfg(any(feature = "test-util", test))]
#[derive(Debug, Default)]
pub struct MockValueProtector {
    counter: std::sync::atomic::AtomicU32,
}

#[cfg(any(feature = "test-util", test))]
impl MockValueProtector {
    /// Creates a new mock protector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Derives a deterministic nonce from the counter bytes (repeated to fill).
    fn nonce_bytes(counter: u32) -> [u8; MOCK_NONCE_SIZE] {
        let counter_bytes = counter.to_le_bytes();
        let mut nonce = [0u8; MOCK_NONCE_SIZE];
        for (i, byte) in nonce.iter_mut().enumerate() {
            *byte = counter_bytes[i % counter_bytes.len()];
        }
        nonce
    }

    /// Reversible keystream transform: `body[i] ^= 0x5A ^ nonce[i % NONCE]`.
    fn mask(nonce: &[u8; MOCK_NONCE_SIZE], body: &mut [u8]) {
        for (i, byte) in body.iter_mut().enumerate() {
            *byte ^= 0x5A ^ nonce[i % MOCK_NONCE_SIZE];
        }
    }
}

#[cfg(any(feature = "test-util", test))]
impl ValueProtector for MockValueProtector {
    fn protect(&self, context: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
        use std::sync::atomic::Ordering;

        let nonce = Self::nonce_bytes(self.counter.fetch_add(1, Ordering::Relaxed));
        let mut out = Vec::with_capacity(MOCK_NONCE_SIZE + 4 + context.len() + plaintext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&u32::try_from(context.len()).expect("context fits in u32").to_le_bytes());
        out.extend_from_slice(context);
        let body_start = out.len();
        for (slice, _) in plaintext.slices() {
            out.extend_from_slice(slice);
        }
        Self::mask(&nonce, &mut out[body_start..]);
        Ok(BytesView::from(out))
    }

    fn unprotect(&self, context: &[u8], protected: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
        let bytes = protected.to_vec();
        let Some(nonce) = bytes.get(..MOCK_NONCE_SIZE) else {
            return Ok(DecodeOutcome::SoftFailure("mock: truncated nonce"));
        };
        let nonce: [u8; MOCK_NONCE_SIZE] = nonce.try_into().expect("MOCK_NONCE_SIZE bytes");
        let rest = &bytes[MOCK_NONCE_SIZE..];
        let Some(len_bytes) = rest.get(..4) else {
            return Ok(DecodeOutcome::SoftFailure("mock: truncated length"));
        };
        let context_len = u32::from_le_bytes(len_bytes.try_into().expect("4 bytes")) as usize;
        let Some(stored_context) = rest.get(4..4 + context_len) else {
            return Ok(DecodeOutcome::SoftFailure("mock: truncated context"));
        };
        if stored_context != context {
            return Ok(DecodeOutcome::SoftFailure("mock: context mismatch"));
        }
        let mut body = rest[4 + context_len..].to_vec();
        Self::mask(&nonce, &mut body);
        Ok(DecodeOutcome::Value(BytesView::from(body)))
    }
}

/// A cache tier that transparently protects values with a [`ValueProtector`].
///
/// It wraps an inner `CacheTier<BytesView, BytesView>` (typically a remote tier
/// holding serialized bytes). On insert it protects the value, binding the storage key
/// as context; on get it recovers it, and an authentication failure — corrupt bytes, a
/// tampered entry, or a value relocated from a different key — surfaces as a cache miss
/// (`Ok(None)`) rather than an error. Each such failure emits a `cache.unprotect_failed`
/// telemetry event so that tampering with the backing store is observable rather than
/// silent.
pub(crate) struct ProtectedTier<S> {
    inner: S,
    protector: Box<dyn ValueProtector>,
    telemetry: CacheTelemetry,
    name: CacheName,
}

impl<S> ProtectedTier<S> {
    pub(crate) fn new(inner: S, protector: Box<dyn ValueProtector>, telemetry: CacheTelemetry, name: CacheName) -> Self {
        Self {
            inner,
            protector,
            telemetry,
            name,
        }
    }
}

impl<S: std::fmt::Debug> std::fmt::Debug for ProtectedTier<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedTier").field("inner", &self.inner).finish_non_exhaustive()
    }
}

impl<S> CacheTier<BytesView, BytesView> for ProtectedTier<S>
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
        // The storage key is bound as context, so a value planted under the
        // wrong key fails to unprotect and is treated as a miss.
        let context = to_contiguous(key);
        match self.protector.unprotect(context.as_ref(), &value)? {
            DecodeOutcome::Value(value) => {
                let mut recovered = CacheEntry::new(value);
                if let Some(ttl) = ttl {
                    recovered.set_ttl(ttl);
                }
                if let Some(cached_at) = cached_at {
                    recovered.ensure_cached_at(cached_at);
                }
                Ok(Some(recovered))
            }
            DecodeOutcome::SoftFailure(_) => {
                self.telemetry.record_unprotect_failure(self.name);
                Ok(None)
            }
        }
    }

    async fn insert(&self, key: BytesView, entry: CacheEntry<BytesView>) -> Result<(), Error> {
        let context = to_contiguous(&key);
        let protected = entry.try_map_value(|value| self.protector.protect(context.as_ref(), &value))?;
        self.inner.insert(key, protected).await
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

    /// A protector whose operations always hard-error, for exercising error propagation.
    struct FailingProtector;

    impl ValueProtector for FailingProtector {
        fn protect(&self, _context: &[u8], _plaintext: &BytesView) -> Result<BytesView, Error> {
            Err(Error::from_message("protect failed"))
        }

        fn unprotect(&self, _context: &[u8], _protected: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
            Err(Error::from_message("unprotect failed"))
        }
    }

    fn tier<S>(inner: S) -> ProtectedTier<S> {
        ProtectedTier::new(inner, Box::new(MockValueProtector::new()), CacheTelemetry::new(), "encrypted-test")
    }

    fn failing_tier<S>(inner: S) -> ProtectedTier<S> {
        ProtectedTier::new(inner, Box::new(FailingProtector), CacheTelemetry::new(), "failing-test")
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
        assert!(format!("{tier:?}").contains("ProtectedTier"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn decrypt_failure_emits_telemetry() {
        use testing_aids::LogCapture;

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let inner = MockCache::<BytesView, BytesView>::new();
        let tier = ProtectedTier::new(
            inner.clone(),
            Box::new(MockValueProtector::new()),
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
        capture.assert_contains(crate::telemetry::attributes::EVENT_UNPROTECT_FAILED);
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

    #[test]
    fn to_contiguous_gathers_every_span() {
        // Single-span: returns the full contents (borrowed).
        let single = view(b"contiguous");
        assert_eq!(to_contiguous(&single).as_ref(), b"contiguous");

        // Multi-span: must concatenate ALL spans, not just the first one.
        let mut multi = BytesView::from(b"first-".to_vec());
        multi.append(BytesView::from(b"second".to_vec()));
        assert_ne!(multi.first_slice().len(), multi.len(), "fixture must be multi-span");
        assert_eq!(
            to_contiguous(&multi).as_ref(),
            b"first-second",
            "to_contiguous must gather every span, not return only the first"
        );
    }

    #[test]
    fn mock_protector_soft_fails_on_malformed_input() {
        let p = MockValueProtector::new();
        let soft = |bytes: Vec<u8>| matches!(p.unprotect(b"context", &BytesView::from(bytes)), Ok(DecodeOutcome::SoftFailure(_)));

        // Too short to hold the nonce prefix.
        assert!(soft(vec![0u8; 4]), "truncated nonce must soft-fail");
        // Nonce present, but no room for the 4-byte length prefix.
        assert!(soft(vec![0xA5u8; MOCK_NONCE_SIZE]), "truncated length must soft-fail");
        // Length prefix declares a 4-byte context, but no context bytes follow.
        let mut declares_missing_context = vec![0xA5u8; MOCK_NONCE_SIZE];
        declares_missing_context.extend_from_slice(&4u32.to_le_bytes());
        assert!(soft(declares_missing_context), "truncated context must soft-fail");
        // A well-formed round-trip must NOT soft-fail.
        let valid = p.protect(b"context", &view(b"value")).expect("protect should succeed");
        assert!(!soft(valid.to_vec()), "a valid round-trip must recover, not soft-fail");
    }
}
