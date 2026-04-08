// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Marker trait for distributed cache tiers that operate on serialized bytes.

use bytesbuf::BytesView;

use crate::CacheTier;

/// A [`CacheTier`] that stores serialized bytes.
///
/// This is a marker trait for cache tiers that communicate over the network
/// (e.g., Redis, Memcached) where keys and values are already serialized to
/// [`BytesView`]. Use the `.serialize()` builder method to insert a serialization
/// boundary before a distributed tier.
pub trait DistributedCacheTier: CacheTier<BytesView, BytesView> {}
