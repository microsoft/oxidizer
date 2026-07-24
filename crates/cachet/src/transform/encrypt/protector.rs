// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ValueProtector`] authenticated-protection contract.

use bytesbuf::BytesView;

use crate::Error;
use crate::transform::DecodeOutcome;

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
