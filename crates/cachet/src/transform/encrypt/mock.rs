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
/// The stored form is `nonce || context_len(8, LE) || context || masked_body`. The
/// nonce comes from a monotonic counter (so repeated `protect` calls of identical input
/// still differ, yet stay reproducible), and `masked_body` is the plaintext combined
/// with a nonce-derived keystream via XOR. It binds the `context`:
/// [`unprotect`](ValueProtector::unprotect) returns [`DecodeOutcome::SoftFailure`]
/// unless the caller's `context` matches the stored one *exactly* — same length and
/// bytes — so a value cannot be recovered under a different key, including one that is a
/// prefix or extension of the original. Truncated or corrupt input soft-fails too,
/// mirroring the [`ValueProtector`] security contract.
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
        let mut out = Vec::with_capacity(MOCK_NONCE_SIZE + 8 + context.len() + plaintext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&(context.len() as u64).to_le_bytes());
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
        let Some(len_bytes) = rest.get(..8) else {
            return Ok(DecodeOutcome::SoftFailure("mock: truncated length"));
        };
        let stored_len = u64::from_le_bytes(len_bytes.try_into().expect("8 bytes, guarded by get(..8) above"));
        // The stored context length must match the caller's exactly, so a context that
        // is a prefix (or extension) of the stored key is rejected outright rather than
        // partially matched. Compared in u64 space to avoid any usize conversion.
        if stored_len != context.len() as u64 {
            return Ok(DecodeOutcome::SoftFailure("mock: context length mismatch"));
        }
        let after_len = &rest[8..];
        let Some(stored_context) = after_len.get(..context.len()) else {
            return Ok(DecodeOutcome::SoftFailure("mock: truncated context"));
        };
        if stored_context != context {
            return Ok(DecodeOutcome::SoftFailure("mock: context mismatch"));
        }
        let mut body = after_len[context.len()..].to_vec();
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
    fn mock_protector_soft_fails_on_malformed_or_mismatched_input() {
        let p = MockValueProtector::new();
        let soft = |bytes: Vec<u8>| matches!(p.unprotect(b"context", &BytesView::from(bytes)), Ok(DecodeOutcome::SoftFailure(_)));

        // Too short to hold the nonce prefix.
        assert!(soft(vec![0u8; 4]), "truncated nonce must soft-fail");
        // Nonce present, but no room for the 8-byte length field.
        assert!(soft(vec![0u8; MOCK_NONCE_SIZE]), "truncated length must soft-fail");
        // Length field declares a 7-byte context, but fewer context bytes follow.
        let mut truncated_ctx = vec![0u8; MOCK_NONCE_SIZE];
        truncated_ctx.extend_from_slice(&7u64.to_le_bytes());
        truncated_ctx.extend_from_slice(b"abc");
        assert!(soft(truncated_ctx), "truncated context must soft-fail");
        // A key of which the read context is a strict prefix must NOT match (relocation).
        let extended = p.protect(b"context-long", &view(b"value")).expect("protect should succeed");
        assert!(soft(extended.to_vec()), "prefix/length-mismatched context must soft-fail");
        // A blob protected under a different (same-length) context must not match.
        let other = p.protect(b"kontext", &view(b"value")).expect("protect should succeed");
        assert!(soft(other.to_vec()), "context mismatch must soft-fail");
        // A well-formed round-trip under the expected context must NOT soft-fail.
        let valid = p.protect(b"context", &view(b"value")).expect("protect should succeed");
        assert!(!soft(valid.to_vec()), "a valid round-trip must recover, not soft-fail");
    }
}
