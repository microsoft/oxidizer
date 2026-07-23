// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A deterministic, crypto-free [`ValueProtector`] test double.

use bytesbuf::BytesView;

use super::ValueProtector;
use crate::Error;
use crate::transform::DecodeOutcome;

/// Length of the mock protector's nonce prefix, in bytes.
const MOCK_NONCE_SIZE: usize = 12;

/// A deterministic, crypto-free [`ValueProtector`] for tests.
///
/// Available with the `test-util` feature. Use it to exercise a
/// [`protect_with`](crate::TransformBuilder::protect_with) pipeline — round-trips, key
/// binding, and unprotect failures — without a real cryptographic library or a source
/// of entropy, keeping tests fast and reproducible.
///
/// The stored form is `nonce || context || masked_body`. The nonce comes from a
/// monotonic counter (so repeated `protect` calls of identical input still differ, yet
/// stay reproducible), and `masked_body` is the plaintext combined with a nonce-derived
/// keystream via XOR. It binds the `context`: [`unprotect`](ValueProtector::unprotect)
/// returns [`DecodeOutcome::SoftFailure`] on a context mismatch, truncation, or
/// corruption, mirroring the [`ValueProtector`] security contract. On `unprotect` the
/// caller supplies the `context`, so its length is known and never stored.
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
#[derive(Debug, Default)]
pub struct MockValueProtector {
    counter: std::sync::atomic::AtomicU32,
}

impl MockValueProtector {
    /// Creates a new mock protector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Derives a deterministic nonce from the counter bytes (repeated to fill).
    #[cfg_attr(test, mutants::skip)] // Test-only mock: no contract on the exact keystream, only that it is deterministic and reversible (verified by round-trip tests).
    fn nonce_bytes(counter: u32) -> [u8; MOCK_NONCE_SIZE] {
        let counter_bytes = counter.to_le_bytes();
        std::array::from_fn(|i| counter_bytes[i % counter_bytes.len()])
    }

    /// Reversible keystream transform: `body[i] ^= 0x5A ^ nonce[i % NONCE]`.
    #[cfg_attr(test, mutants::skip)] // Test-only mock: no contract on the exact keystream, only that it is deterministic and reversible (verified by round-trip tests).
    fn mask(nonce: &[u8; MOCK_NONCE_SIZE], body: &mut [u8]) {
        for (i, byte) in body.iter_mut().enumerate() {
            *byte ^= 0x5A ^ nonce[i % MOCK_NONCE_SIZE];
        }
    }
}

impl ValueProtector for MockValueProtector {
    fn protect(&self, context: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
        use std::sync::atomic::Ordering;

        let nonce = Self::nonce_bytes(self.counter.fetch_add(1, Ordering::Relaxed));
        let mut out = Vec::with_capacity(MOCK_NONCE_SIZE + context.len() + plaintext.len());
        out.extend_from_slice(&nonce);
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
        let nonce: [u8; MOCK_NONCE_SIZE] = nonce
            .try_into()
            .expect("nonce slice is MOCK_NONCE_SIZE bytes, guarded by get(..MOCK_NONCE_SIZE) above");
        let rest = &bytes[MOCK_NONCE_SIZE..];
        // The caller supplies `context`, so its length is known; slice it off directly
        // without any stored length or arithmetic on untrusted input.
        let Some(stored_context) = rest.get(..context.len()) else {
            return Ok(DecodeOutcome::SoftFailure("mock: truncated context"));
        };
        if stored_context != context {
            return Ok(DecodeOutcome::SoftFailure("mock: context mismatch"));
        }
        let mut body = rest[context.len()..].to_vec();
        Self::mask(&nonce, &mut body);
        Ok(DecodeOutcome::Value(BytesView::from(body)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(data: &[u8]) -> BytesView {
        BytesView::from(data.to_vec())
    }

    #[test]
    fn mock_protector_soft_fails_on_malformed_input() {
        let p = MockValueProtector::new();
        let soft = |bytes: Vec<u8>| matches!(p.unprotect(b"context", &BytesView::from(bytes)), Ok(DecodeOutcome::SoftFailure(_)));

        // Too short to hold the nonce prefix.
        assert!(soft(vec![0u8; 4]), "truncated nonce must soft-fail");
        // Nonce present, but fewer bytes remain than the context length.
        assert!(soft(vec![0u8; MOCK_NONCE_SIZE]), "truncated context must soft-fail");
        // A blob protected under a different (same-length) context must not match.
        let other = p.protect(b"kontext", &view(b"value")).expect("protect should succeed");
        assert!(soft(other.to_vec()), "context mismatch must soft-fail");
        // A well-formed round-trip under the expected context must NOT soft-fail.
        let valid = p.protect(b"context", &view(b"value")).expect("protect should succeed");
        assert!(!soft(valid.to_vec()), "a valid round-trip must recover, not soft-fail");
    }
}
