// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authenticated protection of cache values stored in an untrusted tier.
//!
//! This provides only the protection *mechanism* — it carries no cryptographic
//! dependency of its own. [`ValueProtector`] is the pluggable contract: you supply the
//! actual implementation, backed by your approved cryptographic library, and register
//! it with [`protect_with`](crate::SerializeBuilder::protect_with). It is installed as a
//! [`Codec`](crate::Codec) stage in the value pipeline, after serialization, where the
//! storage key is available and bound to each value.
//!
//! See the crate-level "Encryption Boundary" docs for a reference [`ValueProtector`]
//! implementation backed by `SymCrypt` (FIPS-certifiable AES-256-GCM).

mod codec;
#[cfg(any(feature = "test-util", test))]
mod mock;
mod protector;

pub(crate) use codec::ProtectorCodec;
#[cfg(any(feature = "test-util", test))]
pub use mock::MockValueProtector;
pub use protector::{Rejection, Unprotected, ValueProtector};
