// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ProtectedTier`] cache tier that applies a [`ValueProtector`] at the boundary.

use std::borrow::Cow;

use bytesbuf::BytesView;

use super::ValueProtector;
use crate::cache::CacheName;
use crate::telemetry::CacheTelemetry;
use crate::transform::DecodeOutcome;
use crate::{CacheEntry, CacheTier, Error, SizeError};

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

    use super::super::MockValueProtector;
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
        use testing_aids::tracing_logs::Capture;

        let capture = Capture::new();
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
}
