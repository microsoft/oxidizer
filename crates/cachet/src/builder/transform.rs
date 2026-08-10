// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for a type-conversion boundary that applies to the *next* fallback tier.
//!
//! `.transform()` returns a [`TransformBuilder`] — a *pending* boundary holding the
//! pre-transform tier and the codecs. Like [`serialize`](super::serialize), it is
//! deliberately **not** buildable on its own: the transform materializes only when you
//! add a storage tier with [`fallback`](TransformBuilder::fallback), which wraps exactly
//! that tier in a [`TransformAdapter`] and hands back an ordinary [`FallbackBuilder`]. To
//! transform another tier, call `.transform()` again — each `.transform()` is its own
//! boundary applying to the single `.fallback()` that follows.
//!
//! Wrapping each tier independently keeps decoding *below* every fallback junction: an
//! undecodable value in one tier decodes to a miss there, so the fallback chain falls
//! through to the next tier rather than shadowing a good copy. (A single adapter over a
//! chain of tiers would decode *above* the junctions, turning a present-but-undecodable
//! blob into a hard miss and hiding a valid copy in a later tier.)

use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

use cachet_tier::DynamicCache;
use tick::Clock;

use super::buildable::{Buildable, type_name};
use super::cache::CacheBuilder;
use super::fallback::FallbackBuilder;
use super::sealed::{CacheTierBuilder, Sealed};
use crate::telemetry::CacheTelemetry;
use crate::transform::{MakeContext, TransformAdapter, keyless_context};
use crate::{Cache, CacheTier, Codec, Encoder};

/// A pending type-conversion boundary, produced by [`transform`](CacheBuilder::transform).
///
/// Holds the pre-transform tier plus the codecs that convert FROM the user types
/// (`K, V`) TO the storage types (`KT, VT`). Add a storage tier speaking `KT, VT` with
/// [`fallback`](Self::fallback) to materialize the boundary; a `TransformBuilder` on its
/// own is not buildable.
pub struct TransformBuilder<K, V, KT, VT, Pre> {
    pub(super) pre: Pre,
    pub(super) key_encoder: Box<dyn Encoder<K, KT>>,
    pub(super) value_codec: Box<dyn Codec<V, VT>>,
    pub(super) clock: Clock,
    pub(super) telemetry: CacheTelemetry,
    pub(super) stampede_protection: bool,
    pub(super) _phantom: PhantomData<(K, V, KT, VT)>,
}

impl<K, V, KT, VT, Pre: Debug> Debug for TransformBuilder<K, V, KT, VT, Pre> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformBuilder")
            .field("pre", &self.pre)
            .field("K", &std::any::type_name::<K>())
            .field("KT", &std::any::type_name::<KT>())
            .field("V", &std::any::type_name::<V>())
            .field("VT", &std::any::type_name::<VT>())
            .finish_non_exhaustive()
    }
}

fn new_transform_builder<K, V, KT, VT, Pre>(
    pre: Pre,
    key_encoder: impl Encoder<K, KT> + 'static,
    value_codec: impl Codec<V, VT> + 'static,
    clock: Clock,
    telemetry: CacheTelemetry,
    stampede_protection: bool,
) -> TransformBuilder<K, V, KT, VT, Pre> {
    TransformBuilder {
        pre,
        key_encoder: Box::new(key_encoder),
        value_codec: Box::new(value_codec),
        clock,
        telemetry,
        stampede_protection,
        _phantom: PhantomData,
    }
}

// ── .transform() entry points ──

impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Begins a type-conversion boundary for the next fallback tier.
    ///
    /// The codecs convert FROM user types TO storage types:
    /// - `key_encoder`: `K -> KT` (one-directional)
    /// - `value_codec`: `V <-> VT` (bidirectional)
    ///
    /// Add a storage tier speaking `KT, VT` with
    /// [`fallback`](TransformBuilder::fallback). The transform applies to that one tier;
    /// to transform another, call `.transform()` again.
    #[must_use]
    pub fn transform<KT, VT>(
        self,
        key_encoder: impl Encoder<K, KT> + 'static,
        value_codec: impl Codec<V, VT> + 'static,
    ) -> TransformBuilder<K, V, KT, VT, Self>
    where
        KT: Clone + Hash + Eq + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede = self.stampede_protection;
        new_transform_builder(self, key_encoder, value_codec, clock, telemetry, stampede)
    }
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Begins a type-conversion boundary applying to the next fallback tier.
    ///
    /// See [`CacheBuilder::transform`] for the semantics; here the pre-transform tier is
    /// the fallback hierarchy built so far.
    #[must_use]
    pub fn transform<KT, VT>(
        self,
        key_encoder: impl Encoder<K, KT> + 'static,
        value_codec: impl Codec<V, VT> + 'static,
    ) -> TransformBuilder<K, V, KT, VT, Self>
    where
        KT: Clone + Hash + Eq + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede = self.stampede_protection;
        new_transform_builder(self, key_encoder, value_codec, clock, telemetry, stampede)
    }
}

// ── .fallback() — materializes the boundary as one wrapped tier ──

impl<K, V, KT, VT, Pre> TransformBuilder<K, V, KT, VT, Pre> {
    /// Adds the storage tier this boundary transforms (speaks `KT, VT`), returning an
    /// ordinary [`FallbackBuilder`] with the pre-transform tier as primary and the
    /// adapted tier as fallback.
    ///
    /// The transform applies to this one tier only. To transform another tier, call
    /// `.transform()` again on the returned builder.
    #[must_use]
    pub fn fallback<FB>(self, fallback: FB) -> FallbackBuilder<K, V, Pre, TransformTierBuilder<K, V, KT, VT, FB>> {
        let wrapped = TransformTierBuilder {
            inner: fallback,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            make_context: keyless_context(),
            clock: self.clock.clone(),
            telemetry: self.telemetry.clone(),
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        };
        FallbackBuilder {
            name: None,
            primary_builder: self.pre,
            fallback_builder: wrapped,
            clock: self.clock,
            refresh: None,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }
}

/// A per-leaf builder that wraps one storage tier in a [`TransformAdapter`].
///
/// Produced by [`TransformBuilder::fallback`] (and, with `BytesView` storage types, by
/// [`SerializeBuilder::fallback`](super::serialize::SerializeBuilder::fallback)); it
/// carries the codecs plus the key-context function and, at build time, decorates its
/// inner tier so each backing store is converted independently — keeping decoding below
/// every fallback junction.
pub struct TransformTierBuilder<K, V, KT, VT, Inner> {
    pub(super) inner: Inner,
    pub(super) key_encoder: Box<dyn Encoder<K, KT>>,
    pub(super) value_codec: Box<dyn Codec<V, VT>>,
    pub(super) make_context: MakeContext<KT>,
    pub(super) clock: Clock,
    pub(super) telemetry: CacheTelemetry,
    pub(super) stampede_protection: bool,
    pub(super) _phantom: PhantomData<(K, V, KT, VT)>,
}

impl<K, V, KT, VT, Inner: Debug> Debug for TransformTierBuilder<K, V, KT, VT, Inner> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformTierBuilder")
            .field("inner", &self.inner)
            .field("K", &std::any::type_name::<K>())
            .field("KT", &std::any::type_name::<KT>())
            .field("V", &std::any::type_name::<V>())
            .field("VT", &std::any::type_name::<VT>())
            .finish_non_exhaustive()
    }
}

impl<K, V, KT, VT, Inner> Sealed for TransformTierBuilder<K, V, KT, VT, Inner> {}

impl<K, V, KT, VT, Inner> CacheTierBuilder<K, V> for TransformTierBuilder<K, V, KT, VT, Inner>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    Inner: CacheTierBuilder<KT, VT>,
{
}

impl<K, V, KT, VT, Inner> Buildable<K, V> for TransformTierBuilder<K, V, KT, VT, Inner>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    Inner: Buildable<KT, VT>,
{
    type TierOutput = TransformAdapter<K, KT, V, VT, Inner::TierOutput>;

    // A `TransformTierBuilder` is only ever composed as the fallback tier of a
    // `FallbackBuilder`, which drives it through `build_tier`; `build` is required by the
    // trait but never reached, so it is excluded from coverage.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn build(self) -> Cache<K, V> {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        let tier = DynamicCache::new(self.build_tier(clock.clone(), telemetry.clone(), false));

        Cache::new(type_name::<Self::TierOutput>(None), tier, clock, telemetry, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry, fallback: bool) -> Self::TierOutput {
        let inner = self.inner.build_tier(clock, telemetry, fallback);
        TransformAdapter::from_boxed(inner, self.key_encoder, self.value_codec, self.make_context)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cachet_tier::MockCache;

    use super::*;
    use crate::transform::{TransformCodec, TransformEncoder, infallible, infallible_owned};

    #[test]
    fn transform_tier_builder_debug() {
        let builder = Cache::builder::<i32, i32>(Clock::new_frozen())
            .storage(MockCache::<i32, i32>::new())
            .transform(
                TransformEncoder::infallible(|k: &i32| k.to_string()),
                TransformCodec::new(
                    infallible(|v: &i32| v.to_string()),
                    infallible_owned(|v: String| v.parse::<i32>().unwrap_or_default()),
                ),
            )
            .fallback(Cache::builder::<String, String>(Clock::new_frozen()).storage(MockCache::<String, String>::new()));

        let debug = format!("{:?}", builder.fallback_builder);
        assert!(debug.contains("TransformTierBuilder"), "debug output was: {debug}");
    }
}
