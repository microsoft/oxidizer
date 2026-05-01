// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use cachet_tier::CacheEntry;

/// Type alias for insert predicate functions.
type InsertPredicate<V> = Arc<dyn Fn(&CacheEntry<V>) -> bool + Send + Sync>;

/// Policy that determines when values should be inserted into a cache tier.
///
/// The insert policy applies to all inserts into the tier, including direct
/// [`Cache::insert`](crate::Cache::insert) calls, [`Cache::get_or_insert`](crate::Cache::get_or_insert), and promotion from a
/// fallback tier. If the policy rejects an insert, the operation is skipped
/// and a `cache.rejected` telemetry event is recorded, with the operation recorded as `cache.insert`.
///
/// # Examples
///
/// ```
/// use cachet::InsertPolicy;
///
/// // Always insert (default)
/// let policy = InsertPolicy::<String>::always();
///
/// // Never insert
/// let policy = InsertPolicy::<String>::never();
///
/// // Insert based on a condition
/// let policy = InsertPolicy::<String>::when(|entry| entry.value().len() >= 5);
/// ```
#[derive(Debug)]
pub struct InsertPolicy<V>(PolicyType<V>);

enum PolicyType<V> {
    /// Always insert values into the cache tier.
    Always,
    /// Never insert values into the cache tier.
    Never,
    /// Insert based on a boxed predicate that can capture state.
    ///
    /// Use this when you need to capture external state in the predicate.
    /// Has slight overhead from dynamic dispatch.
    When(InsertPredicate<V>),
}

impl<V> Default for InsertPolicy<V> {
    fn default() -> Self {
        Self::always()
    }
}

impl<V> std::fmt::Debug for PolicyType<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Always => write!(f, "Always"),
            Self::Never => write!(f, "Never"),
            Self::When(_) => write!(f, "WhenBoxed(<closure>)"),
        }
    }
}

impl<V> InsertPolicy<V> {
    /// Creates a policy that always inserts values into the cache tier.
    ///
    /// This is the default behavior and maximizes cache hit rates at the cost
    /// of additional writes to the tier.
    #[must_use]
    pub fn always() -> Self {
        Self(PolicyType::Always)
    }

    /// Creates a policy that never inserts values into the cache tier.
    ///
    /// Use this when reads from another tier are already fast enough and you
    /// wanti to avoid write overhead to this tier.
    #[must_use]
    pub fn never() -> Self {
        Self(PolicyType::Never)
    }

    /// Creates a policy using a predicate closure.
    ///
    /// The closure can capture external state if needed.
    ///
    /// ```no_run
    /// use cachet::{Cache, CacheEntry, InsertPolicy};
    /// use tick::Clock;
    ///
    /// let min_len = 3;
    /// let clock = Clock::new_tokio();
    /// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .insert_policy(InsertPolicy::when(
    ///         move |entry: &CacheEntry<String>| entry.value().len() >= min_len,
    ///     ))
    ///     .fallback(l2)
    ///     .build();
    /// ```
    pub fn when<F>(predicate: F) -> Self
    where
        F: Fn(&CacheEntry<V>) -> bool + Send + Sync + 'static,
    {
        Self(PolicyType::When(Arc::new(predicate)))
    }

    /// Returns true if the response should be inserted into the tier.
    #[inline]
    pub(crate) fn should_insert(&self, response: &CacheEntry<V>) -> bool {
        match &self.0 {
            PolicyType::Always => true,
            PolicyType::Never => false,
            PolicyType::When(pred) => pred(response),
        }
    }
}
