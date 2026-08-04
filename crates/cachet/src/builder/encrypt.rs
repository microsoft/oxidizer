// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The `.protect_with()` pipeline stage for the serialization builder.
//!
//! `.protect_with(protector)` is available on a [`SerializeBuilder`] that has not yet
//! been protected. It appends an authenticated-protection stage to the value pipeline
//! and flips the builder's `PROTECTED` type-state to `true`, so the method disappears
//! and protection cannot be configured twice.

use std::marker::PhantomData;
use std::sync::Arc;

use super::serialize::SerializeBuilder;
use crate::ValueProtector;
use crate::transform::ProtectorCodec;

impl<K, V, Pre> SerializeBuilder<K, V, Pre, false> {
    /// Protects values with the given [`ValueProtector`] before they reach any storage
    /// tier, binding each to its storage key.
    ///
    /// Available after [`serialize`](crate::CacheBuilder::serialize), and only if
    /// protection isn't already configured. The protector — backed by your approved
    /// cryptographic library — receives the storage key as context and must bind it (see
    /// the [`ValueProtector`] contract). Keys are never protected; a value that fails
    /// authentication reads as a miss, so the fallback chain continues to the next tier.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .serialize()
    ///     .protect_with(my_protector) // any `ValueProtector` implementation
    ///     .fallback(remote)
    ///     .build();
    /// ```
    #[must_use]
    pub fn protect_with(self, protector: impl ValueProtector + 'static) -> SerializeBuilder<K, V, Pre, true> {
        let protect = ProtectorCodec::new(Arc::new(protector), self.telemetry.clone());
        SerializeBuilder {
            pre: self.pre,
            pool: self.pool,
            protect: Some(Box::new(protect)),
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }
}
