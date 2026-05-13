// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder methods for serialization of cache Key and Value.

use std::hash::Hash;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::transform::TransformBuilder;
use crate::serialize::codec::{PostcardCodec, PostcardEncoder};
use crate::{CacheBuilder, CacheTier, CacheTierBuilder, FallbackBuilder};

impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Applies a serialization boundary that converts keys and values to [`BytesView`](bytesbuf::BytesView).
    ///
    /// Subsequent `.fallback()` tiers must work with `BytesView` keys and values.
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
    pub fn serialize(self) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        let pool = GlobalPool::new();
        self.transform(PostcardEncoder::new(pool.clone()), PostcardCodec::new(pool))
    }
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Applies a serialization boundary on a fallback builder.
    #[must_use]
    pub fn serialize(self) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        let pool = GlobalPool::new();
        self.transform(PostcardEncoder::new(pool.clone()), PostcardCodec::new(pool))
    }
}
