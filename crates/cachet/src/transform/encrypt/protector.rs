// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ValueProtector`] authenticated-protection contract.

use bytesbuf::BytesView;

use crate::Error;

/// Why [`unprotect`](ValueProtector::unprotect) could not recover a value.
///
/// This is a *protection-domain* category — it deliberately lives here rather than on the
/// general [`DecodeOutcome`](crate::DecodeOutcome), which every codec returns and which
/// has no business carrying crypto concepts. Both variants read as a cache miss; they
/// differ only in whether the miss is worth a security signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Rejection {
    /// The stored bytes are not a valid protected envelope — too short, wrong framing, or
    /// an unknown version. Usually benign (corruption or a rolling format migration), so a
    /// fronting tier treats it as a silent miss.
    Malformed,
    /// A well-formed value failed its authentication check — tampered, wrong key, or
    /// relocated to a different key. Security-relevant: a fronting tier records it.
    AuthenticationFailed,
}

/// The outcome of [`ValueProtector::unprotect`].
///
/// Distinguishes a recovered value from the two ways recovery can fail, so a fronting
/// tier can treat them differently even though both read as a cache miss.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Recovered is the common, hot-path variant, so sizing the enum to BytesView is intended; boxing it would add an allocation on every successful unprotect, while Rejected is rare"
)]
pub enum Unprotected {
    /// The value authenticated and was recovered.
    Recovered(BytesView),
    /// The value could not be recovered; see [`Rejection`] for why.
    Rejected(Rejection),
}

/// Authenticated protection of cache values before they reach an untrusted tier.
///
/// Implementations turn a value's plaintext bytes into stored bytes and back, binding
/// a caller-supplied *context* value. The fronting tier passes the entry's storage key
/// as the context, so a value is cryptographically bound to the key it was stored under.
///
/// This trait supplies no implementation of its own: implement it with your
/// organization's approved cryptographic library and register it via
/// [`protect_with`](crate::SerializeBuilder::protect_with). See the crate-level
/// "Encryption Boundary" docs for a reference `SymCrypt`-backed implementation.
///
/// # Security contract
///
/// Implementors **must** bind `context`: [`unprotect`](Self::unprotect) must return
/// [`Unprotected::Rejected`] when the `context` does not match the value supplied to
/// [`protect`](Self::protect). This is what binds each value to its storage key,
/// preventing a value from being relocated to a different key in the backing store.
/// Implementors using a nonce-based scheme are responsible for nonce discipline — use
/// a fresh nonce per [`protect`](Self::protect), or a nonce-misuse-resistant scheme.
///
/// [`unprotect`](Self::unprotect) reports three outcomes:
/// - [`Ok(Unprotected::Recovered(v))`](Unprotected::Recovered) — the value authenticated.
/// - [`Ok(Unprotected::Rejected(reason))`](Unprotected::Rejected) — unrecoverable; the
///   cache treats it as a miss. Use [`Rejection::AuthenticationFailed`] when an
///   authentication check was attempted and failed (tampering, wrong key, relocation),
///   and [`Rejection::Malformed`] when the stored bytes are not a valid envelope
///   (too short, wrong framing, unknown version).
/// - `Err(_)` — the operation could not be attempted (e.g. an unavailable backend); the
///   error propagates to the caller.
pub trait ValueProtector: Send + Sync {
    /// Protects `plaintext`, binding `context`, and returns the stored representation.
    ///
    /// # Errors
    ///
    /// Returns an error if protection cannot be performed.
    fn protect(&self, context: &[u8], plaintext: &BytesView) -> Result<BytesView, Error>;

    /// Recovers a value previously protected under `context`, or reports why it could not
    /// be authenticated.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if the operation could not be attempted. An authentication or
    /// format failure is reported as [`Ok(Unprotected::Rejected(_))`](Unprotected::Rejected).
    fn unprotect(&self, context: &[u8], protected: &BytesView) -> Result<Unprotected, Error>;
}
