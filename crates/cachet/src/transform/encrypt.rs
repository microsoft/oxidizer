// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Encryption codecs for transforming bytes to/from encrypted form.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use bytesbuf::BytesView;

use crate::{Codec, Encoder, Error};

const NONCE_SIZE: usize = 12;

/// A bidirectional codec that encrypts and decrypts bytes using AES-256-GCM.
///
/// `encode` encrypts, prepending a 12-byte random nonce to the ciphertext.
/// `decode` decrypts, expecting the nonce + ciphertext format produced by `encode`.
#[derive(Clone)]
pub struct AesGcmCodec {
    cipher: Aes256Gcm,
}

impl AesGcmCodec {
    /// Creates a new AES-256-GCM codec from a 32-byte key.
    pub fn new(key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(key.into());
        Self { cipher }
    }
}

impl std::fmt::Debug for AesGcmCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AesGcmCodec").finish_non_exhaustive()
    }
}

impl Encoder<BytesView, BytesView> for AesGcmCodec {
    fn encode(&self, value: &BytesView) -> Result<BytesView, Error> {
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        getrandom::getrandom(&mut nonce_bytes).map_err(|e| Error::from_message(format!("failed to generate nonce: {e}")))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, value.first_slice())
            .map_err(|e| Error::from_message(format!("AES-GCM encryption failed: {e}")))?;
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend(ciphertext);
        Ok(result.into())
    }
}

impl Codec<BytesView, BytesView> for AesGcmCodec {
    fn decode(&self, value: &BytesView) -> Result<BytesView, Error> {
        let slice = value.first_slice();
        if slice.len() < NONCE_SIZE {
            return Err(Error::from_message("AES-GCM ciphertext too short: missing nonce"));
        }
        let (nonce_bytes, ciphertext) = slice.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| Error::from_message(format!("AES-GCM decryption failed: {e}")))?;
        Ok(plaintext.into())
    }
}
