// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for a serialization boundary that applies to the *next* fallback tier.
//!
//! `.serialize()` returns a [`SerializeBuilder`] — a *pending* transform holding the
//! pre-transform tier and (with the `encrypt` feature) an optional protector. It is
//! deliberately **not** buildable on its own: the transform materializes only when you
//! add a byte-speaking tier with [`fallback`](SerializeBuilder::fallback), which wraps
//! exactly that tier and hands back an ordinary [`FallbackBuilder`]. To transform another
//! tier, call `.serialize()` again — each `.serialize()` is its own boundary applying to
//! the single `.fallback()` that follows.
//!
//! `.serialize()` is sugar over [`transform`](super::transform): it presets the postcard
//! codecs and a key-context that binds the storage key, so a protector added via
//! [`protect_with`](SerializeBuilder::protect_with) can authenticate each value against
//! its key. Like `.transform()`, it wraps each byte tier independently, so a corrupt or
//! tampered value decodes to a miss there rather than shadowing a good copy in a later
//! tier.

use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tick::Clock;

use super::cache::CacheBuilder;
use super::fallback::FallbackBuilder;
use super::sealed::CacheTierBuilder;
use super::transform::TransformTierBuilder;
use crate::serialize::codec::{PostcardCodec, PostcardEncoder, to_contiguous};
use crate::telemetry::CacheTelemetry;
#[cfg(feature = "encrypt")]
use crate::transform::ChainedCodec;
use crate::transform::{CodecContext, MakeContext};
use crate::{CacheTier, Codec, Encoder};

/// A pending serialization boundary, produced by
/// [`serialize`](CacheBuilder::serialize).
///
/// Holds the pre-transform tier plus (with the `encrypt` feature) an optional protector
/// set via [`protect_with`](Self::protect_with). Add a byte-speaking storage tier with
/// [`fallback`](Self::fallback) to materialize the boundary; a `SerializeBuilder` on its
/// own is not buildable. The `PROTECTED` const parameter is type-state: it flips to
/// `true` after [`protect_with`](Self::protect_with) so protection can be configured at
/// most once per boundary.
pub struct SerializeBuilder<K, V, Pre, const PROTECTED: bool = false> {
    pub(super) pre: Pre,
    pub(super) pool: GlobalPool,
    pub(super) protect: Option<Box<dyn Codec<BytesView, BytesView>>>,
    pub(super) clock: Clock,
    pub(super) telemetry: CacheTelemetry,
    pub(super) stampede_protection: bool,
    pub(super) _phantom: PhantomData<(K, V)>,
}

impl<K, V, Pre: Debug, const PROTECTED: bool> Debug for SerializeBuilder<K, V, Pre, PROTECTED> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerializeBuilder")
            .field("pre", &self.pre)
            .field("protected", &PROTECTED)
            .field("K", &std::any::type_name::<K>())
            .field("V", &std::any::type_name::<V>())
            .finish_non_exhaustive()
    }
}

fn new_serialize_builder<K, V, Pre>(
    pre: Pre,
    clock: Clock,
    telemetry: CacheTelemetry,
    stampede_protection: bool,
) -> SerializeBuilder<K, V, Pre> {
    SerializeBuilder {
        pre,
        pool: GlobalPool::new(),
        protect: None,
        clock,
        telemetry,
        stampede_protection,
        _phantom: PhantomData,
    }
}

/// Builds the value codec for the serialize boundary: postcard serialization, optionally
/// wrapped in the protector (which binds the storage key as associated data).
#[cfg(feature = "encrypt")]
fn compose_value_codec<V>(pool: GlobalPool, protect: Option<Box<dyn Codec<BytesView, BytesView>>>) -> Box<dyn Codec<V, BytesView>>
where
    V: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    match protect {
        Some(protect) => Box::new(ChainedCodec::new(Box::new(PostcardCodec::new(pool)), protect)),
        None => Box::new(PostcardCodec::new(pool)),
    }
}

/// Without the `encrypt` feature there is no protector, so the value codec is plain
/// postcard serialization.
#[cfg(not(feature = "encrypt"))]
fn compose_value_codec<V>(pool: GlobalPool, _protect: Option<Box<dyn Codec<BytesView, BytesView>>>) -> Box<dyn Codec<V, BytesView>>
where
    V: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    Box::new(PostcardCodec::new(pool))
}

// ── .serialize() entry points ──

impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Begins a serialization boundary that converts keys and values to [`BytesView`]
    /// for the next fallback tier.
    ///
    /// Add a byte-speaking storage tier with
    /// [`fallback`](SerializeBuilder::fallback); the value is serialized before it
    /// reaches that tier and deserialized on the way back.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .serialize()
    ///     .fallback(remote)
    ///     .build();
    /// ```
    #[must_use]
    pub fn serialize(self) -> SerializeBuilder<K, V, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede = self.stampede_protection;
        new_serialize_builder(self, clock, telemetry, stampede)
    }
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Begins a serialization boundary applying to the next fallback tier.
    ///
    /// See [`CacheBuilder::serialize`] for the semantics; here the pre-transform tier is
    /// the fallback hierarchy built so far.
    #[must_use]
    pub fn serialize(self) -> SerializeBuilder<K, V, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        let clock = self.clock.clone();
        let telemetry = self.telemetry.clone();
        let stampede = self.stampede_protection;
        new_serialize_builder(self, clock, telemetry, stampede)
    }
}

// ── .fallback() — materializes the boundary as one wrapped tier ──

impl<K, V, Pre, const PROTECTED: bool> SerializeBuilder<K, V, Pre, PROTECTED> {
    /// Adds the byte-speaking storage tier this boundary transforms, returning an
    /// ordinary [`FallbackBuilder`] with the pre-transform tier as primary and the
    /// serialized (and optionally protected) tier as fallback.
    ///
    /// The transform applies to this one tier only. To transform another tier, call
    /// `.serialize()` again on the returned builder.
    #[must_use]
    pub fn fallback<FB>(self, fallback: FB) -> FallbackBuilder<K, V, Pre, TransformTierBuilder<K, V, BytesView, BytesView, FB>>
    where
        K: Serialize + Send + Sync + 'static,
        V: Serialize + DeserializeOwned + Send + Sync + 'static,
    {
        let key_encoder: Box<dyn Encoder<K, BytesView>> = Box::new(PostcardEncoder::new(self.pool.clone()));
        let value_codec = compose_value_codec::<V>(self.pool, self.protect);
        // The storage key is bytes here, so bind it as the codec context: this is what
        // lets a protector authenticate each value against its key.
        let make_context: MakeContext<BytesView> = Box::new(|key: &BytesView| CodecContext::from_key(to_contiguous(key)));

        let wrapped = TransformTierBuilder {
            inner: fallback,
            key_encoder,
            value_codec,
            make_context,
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cachet_tier::MockCache;

    use super::*;
    use crate::Cache;

    #[test]
    fn serialize_fallback_builds_a_transform_tier() {
        let builder = Cache::builder::<String, String>(Clock::new_frozen())
            .storage(MockCache::<String, String>::new())
            .serialize()
            .fallback(Cache::builder::<BytesView, BytesView>(Clock::new_frozen()).storage(MockCache::<BytesView, BytesView>::new()));

        let debug = format!("{:?}", builder.fallback_builder);
        assert!(debug.contains("TransformTierBuilder"), "debug output was: {debug}");
    }
}
