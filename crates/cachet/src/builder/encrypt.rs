// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for applying an authenticated value-protection boundary in the cache
//! pipeline.
//!
//! `.protect_with(protector)` becomes available once a [`TransformBuilder`] has reduced
//! values to [`BytesView`](bytesbuf::BytesView) (typically via
//! [`serialize`](crate::CacheBuilder::serialize)). It produces an
//! [`ProtectedTransformBuilder`], whose post-transform tier chain is wrapped in an
//! internal `ProtectedTier` at build time.

use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

use bytesbuf::BytesView;
use cachet_tier::DynamicCache;
use tick::Clock;

use super::buildable::{Buildable, type_name};
use super::fallback::FallbackBuilder;
use super::sealed::{CacheTierBuilder, Sealed};
use super::transform::TransformBuilder;
use crate::telemetry::CacheTelemetry;
use crate::transform::{ProtectedTier, TransformAdapter, ValueProtector};
use crate::{Codec, Encoder};

/// The builder produced by [`TransformBuilder::protect_with`].
///
/// It mirrors [`TransformBuilder`] but fixes the storage types to
/// [`BytesView`] and carries a value protector. At build time the post-transform tier
/// chain is wrapped in an internal `ProtectedTier`, which protects values and binds
/// each value to its storage key. Add post tiers with [`fallback`](Self::fallback) and
/// finish with [`build`](Self::build), exactly as with `TransformBuilder`.
pub struct ProtectedTransformBuilder<K, V, Pre, Post = ()> {
    pre: Pre,
    post: Post,
    key_encoder: Box<dyn Encoder<K, BytesView>>,
    value_codec: Box<dyn Codec<V, BytesView>>,
    protector: Box<dyn ValueProtector>,
    clock: Clock,
    telemetry: CacheTelemetry,
    stampede_protection: bool,
}

impl<K, V, Pre: Debug, Post: Debug> Debug for ProtectedTransformBuilder<K, V, Pre, Post> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedTransformBuilder")
            .field("pre", &self.pre)
            .field("post", &self.post)
            .field("K", &std::any::type_name::<K>())
            .field("V", &std::any::type_name::<V>())
            .finish_non_exhaustive()
    }
}

// ── .protect_with() on TransformBuilder ──

impl<K, V, Pre, Post> TransformBuilder<K, V, BytesView, BytesView, Pre, Post> {
    /// Protects values with the given [`ValueProtector`](crate::ValueProtector) before
    /// they reach the post-transform tier.
    ///
    /// Available once values are [`BytesView`] (typically after
    /// [`serialize`](crate::CacheBuilder::serialize)). Supply a protector backed by
    /// your approved cryptographic library; it receives the storage key as its context
    /// and must bind it (see the [`ValueProtector`](crate::ValueProtector) contract).
    /// Keys themselves are never protected, and a value that fails authentication is
    /// treated as a cache miss.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use cachet::{Cache, DecodeOutcome, Error, ValueProtector};
    /// use tick::Clock;
    ///
    /// struct MyProtector;
    /// impl ValueProtector for MyProtector {
    ///     fn protect(
    ///         &self,
    ///         context: &[u8],
    ///         plaintext: &bytesbuf::BytesView,
    ///     ) -> Result<bytesbuf::BytesView, Error> {
    /// #       unimplemented!()
    ///         // ... protect with your approved library, binding `context` ...
    ///     }
    ///     fn unprotect(
    ///         &self,
    ///         context: &[u8],
    ///         protected: &bytesbuf::BytesView,
    ///     ) -> Result<DecodeOutcome<bytesbuf::BytesView>, Error> {
    /// #       unimplemented!()
    ///         // ... recover, returning SoftFailure on any authentication failure ...
    ///     }
    /// }
    ///
    /// let clock = Clock::new_tokio();
    /// let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .serialize()
    ///     .protect_with(MyProtector)
    ///     .fallback(remote)
    ///     .build();
    /// ```
    #[must_use]
    pub fn protect_with(self, protector: impl ValueProtector + 'static) -> ProtectedTransformBuilder<K, V, Pre, Post> {
        ProtectedTransformBuilder {
            pre: self.pre,
            post: self.post,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            protector: Box::new(protector),
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
        }
    }
}

// ── .fallback() on ProtectedTransformBuilder ──

impl<K, V, Pre> ProtectedTransformBuilder<K, V, Pre, ()> {
    /// Sets the first post-transform storage tier (speaks encrypted `BytesView`).
    pub fn fallback<FB>(self, fallback: FB) -> ProtectedTransformBuilder<K, V, Pre, FB>
    where
        FB: CacheTierBuilder<BytesView, BytesView>,
    {
        ProtectedTransformBuilder {
            pre: self.pre,
            post: fallback,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            protector: self.protector,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
        }
    }
}

impl<K, V, Pre, Post> ProtectedTransformBuilder<K, V, Pre, Post>
where
    Post: CacheTierBuilder<BytesView, BytesView>,
{
    /// Adds another post-transform fallback tier (speaks encrypted `BytesView`).
    pub fn fallback<FB>(self, fallback: FB) -> ProtectedTransformBuilder<K, V, Pre, FallbackBuilder<BytesView, BytesView, Post, FB>>
    where
        FB: CacheTierBuilder<BytesView, BytesView>,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;

        let post_chain = FallbackBuilder {
            name: None,
            primary_builder: self.post,
            fallback_builder: fallback,
            clock: clock.clone(),
            refresh: None,
            telemetry: telemetry.clone(),
            stampede_protection,
            _phantom: PhantomData,
        };

        ProtectedTransformBuilder {
            pre: self.pre,
            post: post_chain,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            protector: self.protector,
            clock,
            telemetry,
            stampede_protection,
        }
    }
}

// ── Sealed + CacheTierBuilder (allow nesting an encrypted transform) ──

impl<K, V, Pre, Post> Sealed for ProtectedTransformBuilder<K, V, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
}

impl<K, V, Pre, Post> CacheTierBuilder<K, V> for ProtectedTransformBuilder<K, V, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
}

// ── .build() on ProtectedTransformBuilder ──

#[expect(private_bounds, reason = "Buildable is an internal trait")]
impl<K, V, Pre, Post> ProtectedTransformBuilder<K, V, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    Pre: Buildable<K, V>,
    Post: Buildable<BytesView, BytesView>,
{
    /// Builds the full cache hierarchy with the encrypted transform boundary.
    pub fn build(self) -> crate::Cache<K, V> {
        <Self as Buildable<K, V>>::build(self)
    }
}

impl<K, V, Pre, Post> Buildable<K, V> for ProtectedTransformBuilder<K, V, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    Pre: Buildable<K, V>,
    Post: Buildable<BytesView, BytesView>,
{
    type TierOutput = DynamicCache<K, V>;

    fn build(self) -> crate::Cache<K, V> {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede_protection = self.stampede_protection;
        let tier = self.build_tier(clock.clone(), telemetry.clone(), false);

        crate::Cache::new(type_name::<Self::TierOutput>(None), tier, clock, telemetry, stampede_protection)
    }

    fn build_tier(self, clock: Clock, telemetry: CacheTelemetry, fallback: bool) -> Self::TierOutput {
        let pre_tier = self.pre.build_tier(clock.clone(), telemetry.clone(), fallback);

        // Build the post-transform tier chain and wrap it so values are protected
        // (and key-bound) before reaching it.
        let post_tier = self.post.build_tier(clock.clone(), telemetry.clone(), true);
        let protected = ProtectedTier::new(
            post_tier,
            self.protector,
            telemetry.clone(),
            type_name::<ProtectedTier<Post::TierOutput>>(None),
        );
        let adapted = TransformAdapter::from_boxed(protected, self.key_encoder, self.value_codec);

        let fallback = crate::fallback::FallbackCache::new(type_name::<Self::TierOutput>(None), pre_tier, adapted, clock, None, telemetry);

        DynamicCache::new(fallback)
    }
}
