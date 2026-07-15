// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for applying an authenticated-encryption boundary in the cache pipeline.
//!
//! `.encrypt_with(cipher)` becomes available once a [`TransformBuilder`] has reduced
//! values to [`BytesView`](bytesbuf::BytesView) (typically via
//! [`serialize`](crate::CacheBuilder::serialize)). It produces an
//! [`EncryptedTransformBuilder`], whose post-transform tier chain is wrapped in an
//! internal `EncryptedTier` at build time.

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
use crate::transform::{AeadCipher, EncryptedTier, TransformAdapter};
use crate::{Codec, Encoder};

/// The builder produced by [`TransformBuilder::encrypt_with`].
///
/// It mirrors [`TransformBuilder`] but fixes the storage types to
/// [`BytesView`] and carries an authenticated cipher. At build time the
/// post-transform tier chain is wrapped in an internal `EncryptedTier`, which
/// encrypts values and authenticates each value against its storage key. Add post
/// tiers with [`fallback`](Self::fallback) and finish with [`build`](Self::build),
/// exactly as with `TransformBuilder`.
pub struct EncryptedTransformBuilder<K, V, Pre, Post = ()> {
    pre: Pre,
    post: Post,
    key_encoder: Box<dyn Encoder<K, BytesView>>,
    value_codec: Box<dyn Codec<V, BytesView>>,
    cipher: Box<dyn AeadCipher>,
    clock: Clock,
    telemetry: CacheTelemetry,
    stampede_protection: bool,
}

impl<K, V, Pre: Debug, Post: Debug> Debug for EncryptedTransformBuilder<K, V, Pre, Post> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedTransformBuilder")
            .field("pre", &self.pre)
            .field("post", &self.post)
            .field("K", &std::any::type_name::<K>())
            .field("V", &std::any::type_name::<V>())
            .finish_non_exhaustive()
    }
}

// ── .encrypt_with() on TransformBuilder ──

impl<K, V, Pre, Post> TransformBuilder<K, V, BytesView, BytesView, Pre, Post> {
    /// Encrypts values with the given [`AeadCipher`](crate::AeadCipher) before they
    /// reach the post-transform tier.
    ///
    /// Available once values are [`BytesView`] (typically after
    /// [`serialize`](crate::CacheBuilder::serialize)). Supply a cipher backed by your
    /// approved cryptographic library; the cipher receives the storage key as
    /// associated data and must authenticate it (see the
    /// [`AeadCipher`](crate::AeadCipher) contract). Keys themselves are never
    /// encrypted, and a value that fails authentication is treated as a cache miss.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use cachet::{AeadCipher, Cache, DecodeOutcome, Error};
    /// use tick::Clock;
    ///
    /// struct MyCipher;
    /// impl AeadCipher for MyCipher {
    ///     fn encrypt(
    ///         &self,
    ///         aad: &[u8],
    ///         plaintext: &bytesbuf::BytesView,
    ///     ) -> Result<bytesbuf::BytesView, Error> {
    /// #       unimplemented!()
    ///         // ... encrypt with your approved library, authenticating `aad` ...
    ///     }
    ///     fn decrypt(
    ///         &self,
    ///         aad: &[u8],
    ///         ciphertext: &bytesbuf::BytesView,
    ///     ) -> Result<DecodeOutcome<bytesbuf::BytesView>, Error> {
    /// #       unimplemented!()
    ///         // ... decrypt, returning SoftFailure on any authentication failure ...
    ///     }
    /// }
    ///
    /// let clock = Clock::new_tokio();
    /// let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .serialize()
    ///     .encrypt_with(MyCipher)
    ///     .fallback(remote)
    ///     .build();
    /// ```
    #[must_use]
    pub fn encrypt_with(self, cipher: impl AeadCipher + 'static) -> EncryptedTransformBuilder<K, V, Pre, Post> {
        EncryptedTransformBuilder {
            pre: self.pre,
            post: self.post,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            cipher: Box::new(cipher),
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
        }
    }
}

// ── .fallback() on EncryptedTransformBuilder ──

impl<K, V, Pre> EncryptedTransformBuilder<K, V, Pre, ()> {
    /// Sets the first post-transform storage tier (speaks encrypted `BytesView`).
    pub fn fallback<FB>(self, fallback: FB) -> EncryptedTransformBuilder<K, V, Pre, FB>
    where
        FB: CacheTierBuilder<BytesView, BytesView>,
    {
        EncryptedTransformBuilder {
            pre: self.pre,
            post: fallback,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            cipher: self.cipher,
            clock: self.clock,
            telemetry: self.telemetry,
            stampede_protection: self.stampede_protection,
        }
    }
}

impl<K, V, Pre, Post> EncryptedTransformBuilder<K, V, Pre, Post>
where
    Post: CacheTierBuilder<BytesView, BytesView>,
{
    /// Adds another post-transform fallback tier (speaks encrypted `BytesView`).
    pub fn fallback<FB>(self, fallback: FB) -> EncryptedTransformBuilder<K, V, Pre, FallbackBuilder<BytesView, BytesView, Post, FB>>
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

        EncryptedTransformBuilder {
            pre: self.pre,
            post: post_chain,
            key_encoder: self.key_encoder,
            value_codec: self.value_codec,
            cipher: self.cipher,
            clock,
            telemetry,
            stampede_protection,
        }
    }
}

// ── Sealed + CacheTierBuilder (allow nesting an encrypted transform) ──

impl<K, V, Pre, Post> Sealed for EncryptedTransformBuilder<K, V, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
}

impl<K, V, Pre, Post> CacheTierBuilder<K, V> for EncryptedTransformBuilder<K, V, Pre, Post>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
}

// ── .build() on EncryptedTransformBuilder ──

#[expect(private_bounds, reason = "Buildable is an internal trait")]
impl<K, V, Pre, Post> EncryptedTransformBuilder<K, V, Pre, Post>
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

impl<K, V, Pre, Post> Buildable<K, V> for EncryptedTransformBuilder<K, V, Pre, Post>
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

        // Build the post-transform tier chain and wrap it so values are encrypted
        // (and key-authenticated) before reaching it.
        let post_tier = self.post.build_tier(clock.clone(), telemetry.clone(), true);
        let encrypted = EncryptedTier::new(
            post_tier,
            self.cipher,
            telemetry.clone(),
            type_name::<EncryptedTier<Post::TierOutput>>(None),
        );
        let adapted = TransformAdapter::from_boxed(encrypted, self.key_encoder, self.value_codec);

        let fallback = crate::fallback::FallbackCache::new(type_name::<Self::TierOutput>(None), pre_tier, adapted, clock, None, telemetry);

        DynamicCache::new(fallback)
    }
}
