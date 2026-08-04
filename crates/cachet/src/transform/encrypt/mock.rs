// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A deterministic, crypto-free [`ValueProtector`] test double.

use bytesbuf::BytesView;

use super::{Rejection, Unprotected, ValueProtector};
use crate::Error;

/// Length of the mock protector's nonce prefix, in bytes.
const MOCK_NONCE_SIZE: usize = 12;

/// Width of the little-endian context-length field stored after the nonce.
const CONTEXT_LEN_SIZE: usize = size_of::<u64>();

/// A deterministic, crypto-free [`ValueProtector`] for tests.
///
/// Available with the `test-util` feature. Use it to exercise an
/// [`protect_with`](crate::SerializeBuilder::protect_with) pipeline — round-trips, key
/// binding, and unprotect failures — without a real cryptographic library or a source
/// of entropy, keeping tests fast and reproducible.
///
/// The stored form is `nonce || context_len(8, LE) || context || masked_body`. The
/// nonce comes from a monotonic counter (so repeated `protect` calls of identical input
/// still differ, yet stay reproducible), and `masked_body` is the plaintext combined
/// with a nonce-derived keystream via XOR. It binds the `context`:
/// [`unprotect`](ValueProtector::unprotect) returns
/// [`Rejected(AuthenticationFailed)`](Unprotected::Rejected) unless the caller's
/// `context` matches the stored one *exactly* — same length and bytes — so a value
/// cannot be recovered under a different key, including one that is a prefix or extension
/// of the original. Structurally invalid (truncated) input returns
/// [`Rejected(Malformed)`](Unprotected::Rejected), mirroring the [`ValueProtector`]
/// security contract.
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

    /// Returns the next monotonic nonce, so repeated `protect` calls of identical input
    /// still differ yet stay reproducible.
    fn next_nonce(&self) -> [u8; MOCK_NONCE_SIZE] {
        use std::sync::atomic::Ordering;
        Self::nonce_bytes(self.counter.fetch_add(1, Ordering::Relaxed))
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
        // Stored layout: nonce || context_len (u64 LE) || context || masked_body.
        let nonce = self.next_nonce();
        let mut out = Vec::with_capacity(MOCK_NONCE_SIZE + CONTEXT_LEN_SIZE + context.len() + plaintext.len());
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

    fn unprotect(&self, context: &[u8], protected: &BytesView) -> Result<Unprotected, Error> {
        // Peel the stored layout apart segment by segment:
        //   nonce || context_len (u64 LE) || context || masked_body.
        // Structural problems (a blob that isn't a valid envelope) are `Malformed`;
        // a well-formed envelope whose bound context doesn't match is `AuthenticationFailed`.
        let bytes = protected.to_vec();

        let Some((nonce, rest)) = bytes.split_at_checked(MOCK_NONCE_SIZE) else {
            return Ok(Unprotected::Rejected(Rejection::Malformed)); // truncated nonce
        };
        let Some((len_bytes, rest)) = rest.split_at_checked(CONTEXT_LEN_SIZE) else {
            return Ok(Unprotected::Rejected(Rejection::Malformed)); // truncated length
        };

        // The stored context length must match the caller's exactly, so a context that is
        // a prefix (or extension) of the stored key is rejected rather than partially
        // matched. Compared in u64 space to avoid any usize truncation.
        let stored_len = u64::from_le_bytes(
            len_bytes
                .try_into()
                .expect("CONTEXT_LEN_SIZE bytes, guarded by split_at_checked above"),
        );
        if stored_len != context.len() as u64 {
            return Ok(Unprotected::Rejected(Rejection::AuthenticationFailed)); // context length mismatch
        }

        let Some((stored_context, masked_body)) = rest.split_at_checked(context.len()) else {
            return Ok(Unprotected::Rejected(Rejection::Malformed)); // truncated context
        };
        if stored_context != context {
            return Ok(Unprotected::Rejected(Rejection::AuthenticationFailed)); // context mismatch
        }

        let nonce: [u8; MOCK_NONCE_SIZE] = nonce.try_into().expect("MOCK_NONCE_SIZE bytes, guarded by split_at_checked above");
        let mut body = masked_body.to_vec();
        Self::mask(&nonce, &mut body);
        Ok(Unprotected::Recovered(BytesView::from(body)))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn view(data: &[u8]) -> BytesView {
        BytesView::from(data.to_vec())
    }

    #[test]
    fn mock_protector_categorizes_malformed_mismatched_and_valid_input() {
        let p = MockValueProtector::new();
        let kind = |bytes: Vec<u8>| match p.unprotect(b"context", &BytesView::from(bytes)) {
            Ok(Unprotected::Recovered(_)) => None,
            Ok(Unprotected::Rejected(k)) => Some(k),
            Err(e) => panic!("unexpected error: {e}"),
        };

        // Structurally invalid blobs are Malformed (benign): too short to hold the nonce
        // prefix, or the 8-byte length field, or the declared context.
        assert_eq!(kind(vec![0u8; 4]), Some(Rejection::Malformed), "truncated nonce");
        assert_eq!(kind(vec![0u8; MOCK_NONCE_SIZE]), Some(Rejection::Malformed), "truncated length");
        let mut truncated_ctx = vec![0u8; MOCK_NONCE_SIZE];
        truncated_ctx.extend_from_slice(&7u64.to_le_bytes());
        truncated_ctx.extend_from_slice(b"abc");
        assert_eq!(kind(truncated_ctx), Some(Rejection::Malformed), "truncated context");

        // Well-formed envelopes whose bound context doesn't match are AuthenticationFailed:
        // a strict-prefix (length-mismatched) context, or a same-length different context.
        let extended = p.protect(b"context-long", &view(b"value")).expect("protect should succeed");
        assert_eq!(
            kind(extended.to_vec()),
            Some(Rejection::AuthenticationFailed),
            "prefix/length-mismatched context"
        );
        let other = p.protect(b"kontext", &view(b"value")).expect("protect should succeed");
        assert_eq!(kind(other.to_vec()), Some(Rejection::AuthenticationFailed), "context mismatch");

        // A well-formed round-trip under the expected context recovers.
        let valid = p.protect(b"context", &view(b"value")).expect("protect should succeed");
        assert_eq!(kind(valid.to_vec()), None, "a valid round-trip must recover");
    }
}
