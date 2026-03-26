// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transform builder for applying type-conversion boundaries in the cache pipeline.

use std::hash::Hash;
use std::marker::PhantomData;

use cachet_tier::DynamicCache;
use tick::Clock;

use super::buildable::{Buildable, type_name};
use super::cache::CacheBuilder;
use super::fallback::FallbackBuilder;
use super::sealed::{CacheTierBuilder, Sealed};
use crate::fallback::FallbackPromotionPolicy;
use crate::telemetry::{CacheTelemetry, TelemetryConfig};
use crate::wrapper::CacheWrapper;
use crate::{CacheTier, Codec, IdentityCodec, TransformAdapter};

/// Builder that introduces a type-conversion boundary in the cache pipeline.
///
/// - `Pre`: the pre-transform builder (`CacheTierBuilder<K, V>`)
/// - `Post`: the post-transform builder (`CacheTierBuilder<KT, VT>`), starts as `()`
///
/// At build time, both sides are built into tiers, the post-transform tier is wrapped
/// in a `TransformAdapter`, and combined with the pre-transform tier via fallback.
pub struct TransformBuilder<K, V, KT, VT, Pre, Post = ()> {
    pre: Pre,
    post: Post,
    key_encoder: Box<dyn Codec<K, KT>>,
    value_encoder: Box<dyn Codec<V, VT>>,
    value_decoder: Box<dyn Codec<VT, V>>,
    clock: Clock,
    telemetry: TelemetryConfig,
    stampede_protection: bool,
    _phantom: PhantomData<(K, V, KT, VT)>,
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
    /// The codecs convert FROM user types TO storage types:
    /// - `key_encoder`: `K -> KT`
    /// - `value_encoder`: `V -> VT`
    /// - `value_decoder`: `VT -> V`
    ///
    /// Subsequent `.fallback()` tiers must work with `KT, VT`.
    #[must_use]
    pub fn transform<KT, VT>(
        self,
        key_encoder: impl Codec<K, KT> + 'static,
        value_encoder: impl Codec<V, VT> + 'static,
        value_decoder: impl Codec<VT, V> + 'static,
    ) -> TransformBuilder<K, V, KT, VT, Self>
    where
        KT: Clone + Hash + Eq + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        TransformBuilder {
            pre: self,
            post: (),
            key_encoder: Box::new(key_encoder),
            value_encoder: Box::new(value_encoder),
            value_decoder: Box::new(value_decoder),
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
    /// Applies a generic type transform boundary on a fallback builder.
    #[must_use]
    pub fn transform<KT, VT>(
        self,
        key_encoder: impl Codec<K, KT> + 'static,
        value_encoder: impl Codec<V, VT> + 'static,
        value_decoder: impl Codec<VT, V> + 'static,
    ) -> TransformBuilder<K, V, KT, VT, Self>
    where
        KT: Clone + Hash + Eq + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        TransformBuilder {
            pre: self,
            post: (),
            key_encoder: Box::new(key_encoder),
            value_encoder: Box::new(value_encoder),
            value_decoder: Box::new(value_decoder),
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── .serialize() ──

#[cfg(feature = "serialize")]
impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Applies a serialization boundary.
    #[must_use]
    pub fn serialize(
        self,
        key_encoder: impl Codec<K, Vec<u8>> + 'static,
        value_encoder: impl Codec<V, Vec<u8>> + 'static,
        value_decoder: impl Codec<Vec<u8>, V> + 'static,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Self> {
        self.transform(key_encoder, value_encoder, value_decoder)
    }
}

#[cfg(feature = "serialize")]
impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Applies a serialization boundary on a fallback builder.
    #[must_use]
    pub fn serialize(
        self,
        key_encoder: impl Codec<K, Vec<u8>> + 'static,
        value_encoder: impl Codec<V, Vec<u8>> + 'static,
        value_decoder: impl Codec<Vec<u8>, V> + 'static,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Self> {
        self.transform(key_encoder, value_encoder, value_decoder)
    }
}

// ── .fallback() on TransformBuilder ──

impl<K, V, KT, VT, Pre> TransformBuilder<K, V, KT, VT, Pre, ()>
where
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
{
    /// Sets the first post-transform storage tier (speaks `KT, VT`).
    pub fn fallback<FB>(self, fallback: FB) -> TransformBuilder<K, V, KT, VT, Pre, FB>
    where
        FB: CacheTierBuilder<KT, VT>,
    {
        TransformBuilder {
            pre: self.pre,
            post: fallback,
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

impl<K, V, KT, VT, Pre, Post> TransformBuilder<K, V, KT, VT, Pre, Post>
where
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    Post: CacheTierBuilder<KT, VT>,
{
    /// Adds another post-transform fallback tier (speaks `KT, VT`).
    pub fn fallback<FB>(self, fallback: FB) -> TransformBuilder<K, V, KT, VT, Pre, FallbackBuilder<KT, VT, Post, FB>>
    where
        FB: CacheTierBuilder<KT, VT>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;

        let post_chain = FallbackBuilder {
            name: None,
            primary_builder: self.post,
            fallback_builder: fallback,
            policy: FallbackPromotionPolicy::always(),
            clock: clock.clone(),
            refresh: None,
            telemetry: telemetry.clone(),
            stampede_protection,
            _phantom: PhantomData,
        };

        TransformBuilder {
            pre: self.pre,
            post: post_chain,
            key_encoder: self.key_encoder,
            value_encoder: self.value_encoder,
            value_decoder: self.value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── .compress() ──

#[cfg(feature = "compress")]
impl<K, V: Send + Sync + 'static, Pre, Post> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Pre, Post> {
    /// Adds a compression layer. Values are compressed; keys pass through unchanged.
    #[must_use]
    pub fn compress(
        self,
        compress_encoder: impl Codec<Vec<u8>, Vec<u8>> + 'static,
        compress_decoder: impl Codec<Vec<u8>, Vec<u8>> + 'static,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Pre, Post> {
        let TransformBuilder {
            pre,
            post,
            key_encoder,
            value_encoder,
            value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom,
        } = self;

        let new_ve: Box<dyn Codec<V, Vec<u8>>> = Box::new(ChainedCodec {
            first: value_encoder,
            second: Box::new(compress_encoder),
        });

        let new_vd: Box<dyn Codec<Vec<u8>, V>> = Box::new(ChainedCodec {
            first: Box::new(compress_decoder),
            second: value_decoder,
        });

        TransformBuilder {
            pre,
            post,
            key_encoder,
            value_encoder: new_ve,
            value_decoder: new_vd,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── .encrypt() ──

#[cfg(feature = "encrypt")]
impl<K, V: Send + Sync + 'static, Pre, Post> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Pre, Post> {
    /// Adds an encryption layer. Values are encrypted; keys pass through unchanged.
    #[must_use]
    pub fn encrypt(
        self,
        encrypt_encoder: impl Codec<Vec<u8>, Vec<u8>> + 'static,
        encrypt_decoder: impl Codec<Vec<u8>, Vec<u8>> + 'static,
    ) -> TransformBuilder<K, V, Vec<u8>, Vec<u8>, Pre, Post> {
        let TransformBuilder {
            pre,
            post,
            key_encoder,
            value_encoder,
            value_decoder,
            clock,
            telemetry,
            stampede_protection,
            _phantom,
        } = self;

        let new_ve: Box<dyn Codec<V, Vec<u8>>> = Box::new(ChainedCodec {
            first: value_encoder,
            second: Box::new(encrypt_encoder),
        });

        let new_vd: Box<dyn Codec<Vec<u8>, V>> = Box::new(ChainedCodec {
            first: Box::new(encrypt_decoder),
            second: value_decoder,
        });

        TransformBuilder {
            pre,
            post,
            key_encoder,
            value_encoder: new_ve,
            value_decoder: new_vd,
            clock,
            telemetry,
            stampede_protection,
            _phantom: PhantomData,
        }
    }
}

// ── Codec chaining ──

/// Chains two codecs: applies `first` then `second`.
struct ChainedCodec<A, B, C> {
    first: Box<dyn Codec<A, B>>,
    second: Box<dyn Codec<B, C>>,
}

impl<A: Send + Sync, B: Send + Sync, C: Send + Sync> Codec<A, C> for ChainedCodec<A, B, C> {
    fn apply(&self, value: &A) -> Result<C, crate::Error> {
        let intermediate = self.first.apply(value)?;
        self.second.apply(&intermediate)
    }
}

// SAFETY: Send + Sync is guaranteed by Codec: Send + Sync on the boxed fields.
unsafe impl<A, B, C> Send for ChainedCodec<A, B, C> {}
unsafe impl<A, B, C> Sync for ChainedCodec<A, B, C> {}

// ── Sealed + CacheTierBuilder ──

impl<K, V, KT, VT, Pre, Post> Sealed for TransformBuilder<K, V, KT, VT, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
{
}

impl<K, V, KT, VT, Pre, Post> CacheTierBuilder<K, V> for TransformBuilder<K, V, KT, VT, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
{
}

// ── .build() ──

#[expect(private_bounds, reason = "Buildable is an internal trait")]
impl<K, V, KT, VT, Pre, Post> TransformBuilder<K, V, KT, VT, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    Pre: Buildable<K, V>,
    Post: Buildable<KT, VT>,
{
    /// Builds the full cache hierarchy with the transform boundary.
    pub fn build(self) -> crate::Cache<K, V, DynamicCache<K, V>> {
        <Self as Buildable<K, V>>::build(self)
    }
}

// ── Buildable ──

impl<K, V, KT, VT, Pre, Post> Buildable<K, V> for TransformBuilder<K, V, KT, VT, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    KT: Clone + Hash + Eq + Send + Sync + 'static,
    VT: Clone + Send + Sync + 'static,
    Pre: Buildable<K, V>,
    Post: Buildable<KT, VT>,
{
    type Output = DynamicCache<K, V>;
    type TierOutput = TransformAdapter<K, KT, V, VT, Post::TierOutput>;

    fn build(self) -> crate::Cache<K, V, Self::Output> {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone().build();
        let stampede_protection = self.stampede_protection;

        // Build pre-transform tier
        let pre_tier = self.pre.build_tier(clock.clone(), telemetry.clone());

        // Build post-transform tier, wrap in TransformAdapter
        let post_tier = self.post.build_tier(clock.clone(), telemetry.clone());
        let adapted = TransformAdapter::new(post_tier, self.key_encoder, self.value_encoder, self.value_decoder);

        // Combine via fallback: pre_tier is primary, adapted is fallback
        let fallback = crate::fallback::FallbackCache::new(
            type_name::<Self::TierOutput>(None),
            pre_tier,
            adapted,
            FallbackPromotionPolicy::always(),
            clock.clone(),
            None,
            telemetry,
        );

        let dynamic = DynamicCache::new(fallback);
        crate::Cache::new(
            type_name::<crate::Cache<K, V, Self::Output>>(None),
            dynamic,
            clock,
            stampede_protection,
        )
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry) -> Self::TierOutput {
        let post_tier = self.post.build_tier(clock, telemetry);
        TransformAdapter::new(post_tier, self.key_encoder, self.value_encoder, self.value_decoder)
    }
}
