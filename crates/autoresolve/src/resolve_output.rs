//! Output of a single `Resolver::resolve` call: the resolved handle plus the
//! cache tier at which the value lives.
//!
//! The `tier` field flows upward through the resolution recursion so that the
//! caller can decide where to place the resulting value. With the convention
//! that tier 0 is the root resolver and each `scoped()` increases the tier by
//! one, a value's placement tier is the maximum of its dependencies' tiers.

use std::sync::Arc;

/// A handle to a resolved value together with its placement tier.
///
/// `tier` is the level of the resolver whose cache holds the value:
///
/// - tier `0` — the root resolver
/// - tier `n` — `n` scopes deep from the root (i.e. the resolver created by
///   `n` nested `scoped()` calls)
#[derive(Debug)]
pub struct ResolveOutput<O: ?Sized> {
    /// Handle to the resolved value, sharing ownership with the resolver's cache.
    pub value: Arc<O>,
    /// Cache tier at which the value lives.
    pub tier: usize,
}

impl<O: ?Sized> ResolveOutput<O> {
    /// Constructs a new output. Internal helper used by `Resolver`.
    pub(crate) fn new(value: Arc<O>, tier: usize) -> Self {
        Self { value, tier }
    }
}
