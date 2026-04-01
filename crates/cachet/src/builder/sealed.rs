// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A builder that can produce a cache tier.
///
/// This trait is sealed and cannot be implemented outside this crate.
/// It's implemented by `CacheBuilder` and `FallbackBuilder` to enable
/// type-safe cache hierarchy construction.
///
/// # Examples
///
/// ```no_run
/// use cachet::Cache;
/// use tick::Clock;
///
/// let clock = Clock::new_tokio();
/// let cache = Cache::builder::<String, i32>(clock).memory().build();
/// ```
#[expect(private_bounds, reason = "intentionally sealed trait pattern")]
pub trait CacheTierBuilder<K, V>: Sealed {}

pub(crate) trait Sealed {}
