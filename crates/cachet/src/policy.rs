// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Type alias for insert predicate functions.
type InsertPredicate<V> = Arc<dyn Fn(&CacheEntry<V>) -> bool + Send + Sync>;

/// Policy for inserting values from fallback to primary cache.
///
/// When a cache miss occurs in the primary tier and a value is found in the
/// fallback tier, the insert policy determines whether to copy that value
/// back to the primary tier for faster future access.
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
/// // Promote based on a condition
/// let policy = InsertPolicy::<String>::when(|entry| entry.value().len() >= 5);
/// ```
#[derive(Debug, Default)]
pub struct InsertPolicy<V>(PolicyType<V>);

#[derive(Default)]
enum PolicyType<V> {
    /// Always insert values to primary cache.
    #[default]
    Always,
    /// Never insert values to primary cache.
    Never,
    /// Promote based on a boxed predicate that can capture state.
    ///
    /// Use this when you need to capture external state in the predicate.
    /// Has slight overhead from dynamic dispatch.
    When(InsertPredicate<V>),
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
    /// Creates a policy that always inserts values to the primary cache.
    ///
    /// This is the default behavior and maximizes cache hit rates at the cost
    /// of additional writes to the primary tier.
    #[must_use]
    pub fn always() -> Self {
        Self(PolicyType::Always)
    }

    /// Creates a policy that never inserts values to the primary cache.
    ///
    /// Use this when the fallback tier is already fast enough and you want
    /// to avoid write overhead to the primary tier.
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
    ///     .fallback(l2)
    ///     .insert_policy(InsertPolicy::when(
    ///         move |entry: &CacheEntry<String>| entry.value().len() >= min_len,
    ///     ))
    ///     .build();
    /// ```
    pub fn when<F>(predicate: F) -> Self
    where
        F: Fn(&CacheEntry<V>) -> bool + Send + Sync + 'static,
    {
        Self(PolicyType::When(Arc::new(predicate)))
    }

    /// Returns true if the response should be insertd to primary.
    #[inline]
    pub(crate) fn should_insert(&self, response: &CacheEntry<V>) -> bool {
        match &self.0 {
            PolicyType::Always => true,
            PolicyType::Never => false,
            PolicyType::When(pred) => pred(response),
        }
    }
}
