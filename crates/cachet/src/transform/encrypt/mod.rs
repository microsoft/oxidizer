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
//! See the crate-level "Encryption Boundary" docs for a reference [`ValueProtector`]
//! implementation backed by `SymCrypt` (FIPS-certifiable AES-256-GCM).

#[cfg(any(feature = "test-util", test))]
mod mock;
mod protector;
mod tier;

#[cfg(any(feature = "test-util", test))]
pub use mock::MockValueProtector;
pub use protector::ValueProtector;
pub(crate) use tier::ProtectedTier;
