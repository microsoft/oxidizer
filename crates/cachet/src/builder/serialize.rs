// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder methods for serialization of cache Key and Value.

use std::hash::Hash;

use bytesbuf::BytesView;
use bytesbuf::mem::OpaqueMemory;
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
    /// use bytesbuf::mem::{GlobalPool, OpaqueMemory};
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let memory = OpaqueMemory::new(GlobalPool::new());
    /// let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .serialize(memory)
    ///     .fallback(remote)
    ///     .build();
    /// ```
    #[must_use]
    pub fn serialize(self, memory: OpaqueMemory) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        self.transform(PostcardEncoder::new(memory.clone()), PostcardCodec::new(memory))
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
    pub fn serialize(self, memory: OpaqueMemory) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        self.transform(PostcardEncoder::new(memory.clone()), PostcardCodec::new(memory))
    }
}
