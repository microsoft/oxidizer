// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! SymCrypt-backed AES-256-GCM implementation of [`AeadCipher`].
//!
//! Available behind the `symcrypt` feature. Requires the `SymCrypt` library to be
//! present at build and run time (see the crate-level `symcrypt` feature docs).

use bytesbuf::BytesView;
use symcrypt::cipher::BlockCipherType;
use symcrypt::gcm::GcmExpandedKey;

use super::DecodeOutcome;
use super::encrypt::{AeadCipher, to_contiguous};
use crate::Error;

/// Length of the AES-GCM nonce, in bytes. Stored in front of the ciphertext.
const NONCE_SIZE: usize = 12;

/// Length of the AES-GCM authentication tag, in bytes. Stored after the ciphertext.
const TAG_SIZE: usize = 16;

/// An AES-256-GCM [`AeadCipher`] backed by `SymCrypt`.
///
/// `encrypt` writes a fresh random 12-byte nonce in front of the ciphertext
/// (`nonce || ciphertext || tag`) and authenticates the AAD supplied by the
/// encrypted tier (the storage key). Decryption failures — truncation, corruption,
/// tag mismatch, AAD mismatch, or the wrong key — are reported as
/// [`DecodeOutcome::SoftFailure`], so an undecryptable entry is treated as a cache
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
pub struct Aes256GcmCipher {
    key: GcmExpandedKey,
}

impl Aes256GcmCipher {
    /// Creates a new `SymCrypt` AES-256-GCM cipher from a 32-byte key.
    ///
    /// # Panics
    ///
    /// Panics if `SymCrypt` key expansion fails, which cannot happen for a valid
    /// 32-byte AES-256 key (the only failure mode is an unsupported key length).
    #[must_use]
    pub fn new(key: &[u8; 32]) -> Self {
        let key = GcmExpandedKey::new(key, BlockCipherType::AesBlock)
            .expect("SymCrypt AES-256-GCM key expansion cannot fail for a valid 32-byte key");
        Self { key }
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
        let nonce = generate_nonce()?;

        // Gather the plaintext into a single mutable buffer (the one unavoidable
        // copy), encrypt it in place, then assemble `nonce || ciphertext || tag`.
        let mut buffer = to_contiguous(plaintext).into_owned();
        let mut tag = [0u8; TAG_SIZE];
        self.key.encrypt_in_place(&nonce, aad, &mut buffer, &mut tag);

        let mut result = Vec::with_capacity(NONCE_SIZE + buffer.len() + TAG_SIZE);
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&buffer);
        result.extend_from_slice(&tag);
        Ok(result.into())
    }

    fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
        let bytes = to_contiguous(ciphertext);
        if bytes.len() < NONCE_SIZE + TAG_SIZE {
            return Ok(DecodeOutcome::SoftFailure("SymCrypt AES-GCM ciphertext too short"));
        }

        let (nonce, rest) = bytes.split_at(NONCE_SIZE);
        let (body, tag) = rest.split_at(rest.len() - TAG_SIZE);
        let nonce: &[u8; NONCE_SIZE] = nonce.try_into().expect("split_at(NONCE_SIZE) yields exactly 12 bytes");

        let mut buffer = body.to_vec();
        match self.key.decrypt_in_place(nonce, aad, &mut buffer, tag) {
            Ok(()) => Ok(DecodeOutcome::Value(buffer.into())),
            Err(_) => Ok(DecodeOutcome::SoftFailure("SymCrypt AES-GCM decryption failed")),
        }
    }
}

/// Generates a fresh random 12-byte nonce.
///
/// Excluded from coverage: `getrandom::fill` only errors if the OS RNG is
/// unavailable, which cannot be exercised deterministically in a test.
#[cfg_attr(coverage_nightly, coverage(off))]
fn generate_nonce() -> Result<[u8; NONCE_SIZE], Error> {
    let mut nonce = [0u8; NONCE_SIZE];
    getrandom::fill(&mut nonce).map_err(|e| Error::from_message(format!("failed to generate nonce: {e}")))?;
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [42u8; 32];
    const AAD: &[u8] = b"cache-key";

    fn view(data: &[u8]) -> BytesView {
        BytesView::from(data.to_vec())
    }

    fn is_soft_failure<T>(outcome: &Result<DecodeOutcome<T>, Error>) -> bool {
        matches!(outcome, Ok(DecodeOutcome::SoftFailure(_)))
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let plaintext = view(b"the quick brown fox");

        let encrypted = cipher.encrypt(AAD, &plaintext).expect("encrypt should succeed");
        assert_ne!(encrypted.to_vec(), plaintext.to_vec(), "ciphertext must differ from plaintext");

        let outcome = cipher.decrypt(AAD, &encrypted).expect("decrypt should not hard-error");
        assert!(matches!(outcome, DecodeOutcome::Value(v) if v.to_vec() == plaintext.to_vec()));
    }

    #[test]
    fn decrypt_with_wrong_aad_is_soft_failure() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let encrypted = cipher.encrypt(b"key-A", &view(b"secret")).expect("encrypt should succeed");
        assert!(
            is_soft_failure(&cipher.decrypt(b"key-B", &encrypted)),
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
        assert!(
            is_soft_failure(&cipher.decrypt(AAD, &view(&[0u8; NONCE_SIZE]))),
            "truncated input should be a soft failure"
        );
    }

    #[test]
    fn decrypt_tampered_ciphertext_is_soft_failure() {
        let cipher = Aes256GcmCipher::new(&KEY);
        let mut encrypted = cipher.encrypt(AAD, &view(b"secret")).expect("encrypt should succeed").to_vec();
        *encrypted.last_mut().expect("ciphertext is non-empty") ^= 0x01;
        assert!(
            is_soft_failure(&cipher.decrypt(AAD, &BytesView::from(encrypted))),
            "tampered ciphertext should be a soft failure"
        );
    }

    #[test]
    fn decrypt_with_wrong_key_is_soft_failure() {
        let encrypted = Aes256GcmCipher::new(&KEY)
            .encrypt(AAD, &view(b"secret"))
            .expect("encrypt should succeed");
        let other = Aes256GcmCipher::new(&[7u8; 32]);
        assert!(
            is_soft_failure(&other.decrypt(AAD, &encrypted)),
            "wrong key should be a soft failure"
        );
    }

    #[test]
    fn round_trip_over_multi_span_plaintext() {
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
}
