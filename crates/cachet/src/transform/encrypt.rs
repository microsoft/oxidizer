// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Encryption codecs for transforming bytes to/from encrypted form.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};

use crate::{Codec, Error};

/// A codec that encrypts bytes using AES-256-GCM.
///
/// Each encryption prepends a 12-byte nonce to the ciphertext.
/// The nonce is randomly generated per call.
#[derive(Clone)]
pub struct AesGcmEncoder {
    cipher: Aes256Gcm,
}

impl AesGcmEncoder {
    /// Creates a new AES-256-GCM encoder from a 32-byte key.
    pub fn new(key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(key.into());
        Self { cipher }
    }
}

impl std::fmt::Debug for AesGcmEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AesGcmEncoder").finish_non_exhaustive()
    }
}

impl Codec<Vec<u8>, Vec<u8>> for AesGcmEncoder {
    fn apply(&self, value: &Vec<u8>) -> Result<Vec<u8>, Error> {
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        getrandom::getrandom(&mut nonce_bytes).map_err(|e| Error::from_message(format!("failed to generate nonce: {e}")))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, value.as_slice())
            .map_err(|e| Error::from_message(format!("AES-GCM encryption failed: {e}")))?;
        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend(ciphertext);
        Ok(result)
    }
}

/// A codec that decrypts AES-256-GCM encrypted bytes.
///
/// Expects input to be nonce (12 bytes) + ciphertext, as produced by [`AesGcmEncoder`].
#[derive(Clone)]
pub struct AesGcmDecoder {
    cipher: Aes256Gcm,
}

impl AesGcmDecoder {
    /// Creates a new AES-256-GCM decoder from a 32-byte key.
    pub fn new(key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(key.into());
        Self { cipher }
    }
}

impl std::fmt::Debug for AesGcmDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AesGcmDecoder").finish_non_exhaustive()
    }
}

const NONCE_SIZE: usize = 12;

impl Codec<Vec<u8>, Vec<u8>> for AesGcmDecoder {
    fn apply(&self, value: &Vec<u8>) -> Result<Vec<u8>, Error> {
        if value.len() < NONCE_SIZE {
            return Err(Error::from_message("AES-GCM ciphertext too short: missing nonce"));
        }
        let (nonce_bytes, ciphertext) = value.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| Error::from_message(format!("AES-GCM decryption failed: {e}")))
    }
}
