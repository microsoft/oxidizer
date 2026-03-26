// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::Hash;
use std::marker::PhantomData;

use cachet_tier::DynamicCache;
use tick::Clock;

use super::buildable::{Buildable, type_name};
use super::cache::CacheBuilder;
use super::fallback::FallbackBuilder;
use super::sealed::{CacheTierBuilder, Sealed};
use crate::fallback::{FallbackCache, FallbackPromotionPolicy};
use crate::telemetry::{CacheTelemetry, TelemetryConfig};
use crate::wrapper::CacheWrapper;
use crate::{Cache, CacheTier, Codec, IdentityCodec, TransformAdapter, TransformCodec};

// Phase markers — enforce ordering at compile time.
/// Phase after a generic `.transform()` call.
pub struct Transformed;
/// Phase after `.serialize()` — enables `.compress()` and `.encrypt()`.
pub struct Serialized;
/// Phase after `.compress()` — enables `.encrypt()` only.
pub struct Compressed;
/// Phase after `.encrypt()` — no more transforms, only `.fallback()` and `.build()`.
pub struct Encrypted;

/// Builder for constructing a cache with a transform boundary.
///
/// Created by calling `.transform()` or `.serialize()` on a `CacheBuilder` or `FallbackBuilder`.
/// Post-transform fallback tiers are accumulated internally. On `.build()`, the post-transform
/// chain is wrapped in a single `TransformAdapter` and attached as a fallback to the
/// pre-transform builder.
pub struct TransformBuilder<K, V, KT, VT, PreBuilder, KE, VE, VD, PostBuilder, Phase> {
    pre_transform: PreBuilder,
    post_transform: PostBuilder,
    key_encoder: KE,
    value_encoder: VE,
    value_decoder: VD,
    clock: Clock,
    telemetry: TelemetryConfig,
    stampede_protection: bool,
    _phantom: PhantomData<(K, V, KT, VT, Phase)>,
}

// ── .transform() on CacheBuilder ──

impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Applies a generic type transform boundary.
    ///
    /// All subsequent `.fallback()` tiers will work with the transformed types `KT, VT`.
    /// Can be called multiple times in the `Transformed` phase for chained type changes.
    pub fn transform<KT, VT, KE, VE, VD>(
        self,
        key_encoder: KE,
        value_encoder: VE,
        value_decoder: VD,
    ) -> TransformBuilder<K, V, KT, VT, Self, KE, VE, VD, (), Transformed>
    where
        KT: Clone + Hash + Eq + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
        KE: Codec<K, KT>,
        VE: Codec<V, VT>,
        VD: Codec<VT, V>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        TransformBuilder {
            pre_transform: self,
            post_transform: (),
            key_encoder,
            value_encoder,
            value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── .transform() on FallbackBuilder ──

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Applies a generic type transform boundary.
    pub fn transform<KT, VT, KE, VE, VD>(
        self,
        key_encoder: KE,
        value_encoder: VE,
        value_decoder: VD,
    ) -> TransformBuilder<K, V, KT, VT, Self, KE, VE, VD, (), Transformed>
    where
        KT: Clone + Hash + Eq + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
        KE: Codec<K, KT>,
        VE: Codec<V, VT>,
        VD: Codec<VT, V>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        TransformBuilder {
            pre_transform: self,
            post_transform: (),
            key_encoder,
            value_encoder,
            value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── .serialize() on CacheBuilder ──

#[cfg(feature = "serialize")]
impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Applies a serialization boundary using the given codecs.
    ///
    /// Keys and values are serialized to `Vec<u8>`. All subsequent `.fallback()` tiers
    /// must work with `Vec<u8>` keys and values.
    pub fn serialize<KE, VE, VD>(
        self,
        key_encoder: KE,
        value_encoder: VE,
        value_decoder: VD,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Self, KE, VE, VD, (), Serialized>
    where
        KE: Codec<K, Vec<u8>>,
        VE: Codec<V, Vec<u8>>,
        VD: Codec<Vec<u8>, V>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        TransformBuilder {
            pre_transform: self,
            post_transform: (),
            key_encoder,
            value_encoder,
            value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── .serialize() on FallbackBuilder ──

#[cfg(feature = "serialize")]
impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Applies a serialization boundary using the given codecs.
    pub fn serialize<KE, VE, VD>(
        self,
        key_encoder: KE,
        value_encoder: VE,
        value_decoder: VD,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Self, KE, VE, VD, (), Serialized>
    where
        KE: Codec<K, Vec<u8>>,
        VE: Codec<V, Vec<u8>>,
        VD: Codec<Vec<u8>, V>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        TransformBuilder {
            pre_transform: self,
            post_transform: (),
            key_encoder,
            value_encoder,
            value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── Transformed phase: .transform() again, .serialize(), .fallback() ──

impl<K, V, KT, VT, PreBuilder, KE, VE, VD, PostBuilder> TransformBuilder<K, V, KT, VT, PreBuilder, KE, VE, VD, PostBuilder, Transformed>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    KE: Codec<K, KT>,
    VE: Codec<V, VT>,
    VD: Codec<VT, V>,
{
    /// Adds a post-transform fallback tier.
    ///
    /// The fallback tier must work with the transformed types `KT, VT`.
    pub fn fallback<FB>(
        self,
        fallback: FB,
    ) -> TransformBuilder<K, V, KT, VT, PreBuilder, KE, VE, VD, PostTransformTier<PostBuilder, FB>, Transformed>
    where
        FB: CacheTierBuilder<KT, VT>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: PostTransformTier {
                previous: self.post_transform,
                tier: fallback,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── Serialized phase: .compress(), .encrypt(), .fallback() ──

impl<K, V, PreBuilder, KE, VE, VD, PostBuilder> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, PostBuilder, Serialized>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KE: Codec<K, Vec<u8>>,
    VE: Codec<V, Vec<u8>>,
    VD: Codec<Vec<u8>, V>,
{
    /// Adds a post-transform fallback tier.
    pub fn fallback<FB>(
        self,
        fallback: FB,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, PostTransformTier<PostBuilder, FB>, Serialized>
    where
        FB: CacheTierBuilder<Vec<u8>, Vec<u8>>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: PostTransformTier {
                previous: self.post_transform,
                tier: fallback,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }

    /// Adds a compression layer to the transform pipeline.
    ///
    /// Values are compressed after serialization and before encryption.
    /// Keys pass through unchanged via `IdentityCodec`.
    #[cfg(feature = "compress")]
    pub fn compress<CE, CD>(
        self,
        encoder: CE,
        decoder: CD,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, CompressLayer<PostBuilder, CE, CD>, Compressed>
    where
        CE: Codec<Vec<u8>, Vec<u8>>,
        CD: Codec<Vec<u8>, Vec<u8>>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: CompressLayer {
                inner: self.post_transform,
                encoder,
                decoder,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }

    /// Adds an encryption layer to the transform pipeline.
    ///
    /// Values are encrypted after serialization. Keys pass through unchanged.
    #[cfg(feature = "encrypt")]
    pub fn encrypt<EE, ED>(
        self,
        encoder: EE,
        decoder: ED,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, EncryptLayer<PostBuilder, EE, ED>, Encrypted>
    where
        EE: Codec<Vec<u8>, Vec<u8>>,
        ED: Codec<Vec<u8>, Vec<u8>>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: EncryptLayer {
                inner: self.post_transform,
                encoder,
                decoder,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── Compressed phase: .encrypt(), .fallback() ──

impl<K, V, PreBuilder, KE, VE, VD, PostBuilder> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, PostBuilder, Compressed>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KE: Codec<K, Vec<u8>>,
    VE: Codec<V, Vec<u8>>,
    VD: Codec<Vec<u8>, V>,
{
    /// Adds a post-transform fallback tier.
    pub fn fallback<FB>(
        self,
        fallback: FB,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, PostTransformTier<PostBuilder, FB>, Compressed>
    where
        FB: CacheTierBuilder<Vec<u8>, Vec<u8>>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: PostTransformTier {
                previous: self.post_transform,
                tier: fallback,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }

    /// Adds an encryption layer after compression.
    #[cfg(feature = "encrypt")]
    pub fn encrypt<EE, ED>(
        self,
        encoder: EE,
        decoder: ED,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, EncryptLayer<PostBuilder, EE, ED>, Encrypted>
    where
        EE: Codec<Vec<u8>, Vec<u8>>,
        ED: Codec<Vec<u8>, Vec<u8>>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: EncryptLayer {
                inner: self.post_transform,
                encoder,
                decoder,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── Encrypted phase: .fallback() only ──

impl<K, V, PreBuilder, KE, VE, VD, PostBuilder> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, PostBuilder, Encrypted>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KE: Codec<K, Vec<u8>>,
    VE: Codec<V, Vec<u8>>,
    VD: Codec<Vec<u8>, V>,
{
    /// Adds a post-transform fallback tier.
    pub fn fallback<FB>(
        self,
        fallback: FB,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, PreBuilder, KE, VE, VD, PostTransformTier<PostBuilder, FB>, Encrypted>
    where
        FB: CacheTierBuilder<Vec<u8>, Vec<u8>>,
    {
        TransformBuilder {
            pre_transform: self.pre_transform,
            post_transform: PostTransformTier {
                previous: self.post_transform,
                tier: fallback,
            },
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── Build: available on all phases once there's at least one post-transform fallback ──

impl<K, V, KT, VT, PreBuilder, KE, VE, VD, PostInner, PostFB, Phase>
    TransformBuilder<K, V, KT, VT, PreBuilder, KE, VE, VD, PostTransformTier<PostInner, PostFB>, Phase>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    PreBuilder: Buildable<K, V>,
    KE: Codec<K, KT> + 'static,
    VE: Codec<V, VT> + 'static,
    VD: Codec<VT, V> + 'static,
    PostTransformTier<PostInner, PostFB>: PostBuildable<KT, VT>,
{
    /// Builds the full cache hierarchy.
    ///
    /// The post-transform fallback chain is wrapped in a single `TransformAdapter`
    /// and attached as a fallback to the pre-transform builder.
    pub fn build(self) -> Cache<K, V, DynamicCache<K, V>> {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone().build();

        // Build the post-transform chain
        let post_tier = self.post_transform.build_post_tier(clock.clone(), telemetry.clone());

        // Wrap in TransformAdapter
        let adapted = TransformAdapter::new(post_tier, self.key_encoder, self.value_encoder, self.value_decoder);

        // Build the pre-transform tier and add the adapted tier as a fallback
        let pre_tier = self.pre_transform.build_tier(clock.clone(), telemetry.clone());

        let fallback_cache = FallbackCache::new(
            type_name::<Self>(None),
            pre_tier,
            adapted,
            FallbackPromotionPolicy::always(),
            clock.clone(),
            None,
            telemetry,
        );

        let dynamic = DynamicCache::new(fallback_cache);
        Cache::new(type_name::<Self>(None), dynamic, clock, self.stampede_protection)
    }
}

// ── Post-transform tier accumulation ──

/// Wrapper for accumulating post-transform tiers.
pub struct PostTransformTier<Previous, Tier> {
    previous: Previous,
    tier: Tier,
}

/// Internal trait for building the post-transform chain.
pub(crate) trait PostBuildable<K, V> {
    type Output: CacheTier<K, V> + Send + Sync + 'static;
    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output;
}

// Single post-transform tier (first one added after the boundary).
impl<KT, VT, FB> PostBuildable<KT, VT> for PostTransformTier<(), FB>
where
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    FB: Buildable<KT, VT>,
{
    type Output = FB::TierOutput;

    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output {
        self.tier.build_tier(clock, telemetry)
    }
}

// Chained post-transform tiers (second, third, etc.).
impl<KT, VT, PrevInner, PrevTier, FB> PostBuildable<KT, VT> for PostTransformTier<PostTransformTier<PrevInner, PrevTier>, FB>
where
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    PostTransformTier<PrevInner, PrevTier>: PostBuildable<KT, VT>,
    FB: Buildable<KT, VT>,
{
    type Output = FallbackCache<KT, VT, <PostTransformTier<PrevInner, PrevTier> as PostBuildable<KT, VT>>::Output, FB::TierOutput>;

    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output {
        let primary = self.previous.build_post_tier(clock.clone(), telemetry.clone());
        let fallback = self.tier.build_tier(clock.clone(), telemetry.clone());

        FallbackCache::new(
            type_name::<Self::Output>(None),
            primary,
            fallback,
            FallbackPromotionPolicy::always(),
            clock,
            None,
            telemetry,
        )
    }
}

// ── Compress/Encrypt layer types ──

/// Wraps a post-transform chain with a compression `TransformAdapter` at build time.
#[cfg(feature = "compress")]
pub struct CompressLayer<Inner, CE, CD> {
    pub(crate) inner: Inner,
    pub(crate) encoder: CE,
    pub(crate) decoder: CD,
}

/// Wraps a post-transform chain with an encryption `TransformAdapter` at build time.
#[cfg(feature = "encrypt")]
pub struct EncryptLayer<Inner, EE, ED> {
    pub(crate) inner: Inner,
    pub(crate) encoder: EE,
    pub(crate) decoder: ED,
}

// PostBuildable for CompressLayer: wraps inner chain in compression TransformAdapter.
#[cfg(feature = "compress")]
impl<Inner, CE, CD> PostBuildable<Vec<u8>, Vec<u8>> for CompressLayer<Inner, CE, CD>
where
    Inner: PostBuildable<Vec<u8>, Vec<u8>>,
    CE: Codec<Vec<u8>, Vec<u8>> + 'static,
    CD: Codec<Vec<u8>, Vec<u8>> + 'static,
{
    type Output = TransformAdapter<Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Inner::Output, IdentityCodec, CE, CD>;

    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output {
        let inner_tier = self.inner.build_post_tier(clock, telemetry);
        TransformAdapter::new(inner_tier, IdentityCodec, self.encoder, self.decoder)
    }
}

// PostBuildable for EncryptLayer: wraps inner chain in encryption TransformAdapter.
#[cfg(feature = "encrypt")]
impl<Inner, EE, ED> PostBuildable<Vec<u8>, Vec<u8>> for EncryptLayer<Inner, EE, ED>
where
    Inner: PostBuildable<Vec<u8>, Vec<u8>>,
    EE: Codec<Vec<u8>, Vec<u8>> + 'static,
    ED: Codec<Vec<u8>, Vec<u8>> + 'static,
{
    type Output = TransformAdapter<Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Inner::Output, IdentityCodec, EE, ED>;

    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output {
        let inner_tier = self.inner.build_post_tier(clock, telemetry);
        TransformAdapter::new(inner_tier, IdentityCodec, self.encoder, self.decoder)
    }
}

// PostBuildable for PostTransformTier wrapping a CompressLayer.
#[cfg(feature = "compress")]
impl<CompressInner, CE, CD, FB> PostBuildable<Vec<u8>, Vec<u8>> for PostTransformTier<CompressLayer<CompressInner, CE, CD>, FB>
where
    CompressLayer<CompressInner, CE, CD>: PostBuildable<Vec<u8>, Vec<u8>>,
    FB: Buildable<Vec<u8>, Vec<u8>>,
{
    type Output =
        FallbackCache<Vec<u8>, Vec<u8>, <CompressLayer<CompressInner, CE, CD> as PostBuildable<Vec<u8>, Vec<u8>>>::Output, FB::TierOutput>;

    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output {
        let primary = self.previous.build_post_tier(clock.clone(), telemetry.clone());
        let fallback = self.tier.build_tier(clock.clone(), telemetry.clone());

        FallbackCache::new(
            type_name::<Self::Output>(None),
            primary,
            fallback,
            FallbackPromotionPolicy::always(),
            clock,
            None,
            telemetry,
        )
    }
}

// PostBuildable for PostTransformTier wrapping an EncryptLayer.
#[cfg(feature = "encrypt")]
impl<EncryptInner, EE, ED, FB> PostBuildable<Vec<u8>, Vec<u8>> for PostTransformTier<EncryptLayer<EncryptInner, EE, ED>, FB>
where
    EncryptLayer<EncryptInner, EE, ED>: PostBuildable<Vec<u8>, Vec<u8>>,
    FB: Buildable<Vec<u8>, Vec<u8>>,
{
    type Output =
        FallbackCache<Vec<u8>, Vec<u8>, <EncryptLayer<EncryptInner, EE, ED> as PostBuildable<Vec<u8>, Vec<u8>>>::Output, FB::TierOutput>;

    fn build_post_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::Output {
        let primary = self.previous.build_post_tier(clock.clone(), telemetry.clone());
        let fallback = self.tier.build_tier(clock.clone(), telemetry.clone());

        FallbackCache::new(
            type_name::<Self::Output>(None),
            primary,
            fallback,
            FallbackPromotionPolicy::always(),
            clock,
            None,
            telemetry,
        )
    }
}
