// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use routerama_build::trie::{Leaf, VarPlan};
use smallvec::SmallVec;

use crate::route_match::RouteMatch;

/// Captures retained inline in each match.
///
/// Four covers the common route shapes measured by the dynamic capture-count
/// benchmarks. The paired four/five-capture cases track the allocation
/// boundary: increasing this value enlarges every match, while decreasing it
/// makes more requests allocate.
pub(crate) const INLINE_CAPTURES: usize = 4;

/// A matched route, its captures, and its attached value.
pub struct RawMatch<'p, T = ()> {
    pub(crate) leaf: &'p Leaf,
    pub(crate) values: SmallVec<[&'p str; INLINE_CAPTURES]>,
    pub(crate) value: &'p T,
}

impl<T> Clone for RawMatch<'_, T> {
    fn clone(&self) -> Self {
        Self {
            leaf: self.leaf,
            values: self.values.clone(),
            value: self.value,
        }
    }
}

impl<T> fmt::Debug for RawMatch<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMatch")
            .field("name", &self.name())
            .field("captures", &self.captures().collect::<SmallVec<[_; INLINE_CAPTURES]>>())
            .finish_non_exhaustive()
    }
}

impl<'p, T> RouteMatch<'p> for RawMatch<'p, T> {
    #[inline]
    fn name(&self) -> &str {
        &self.leaf.name
    }

    #[inline]
    fn capture(&self, name: &str) -> Option<&'p str> {
        // Captures use their original template names, not generated field names.
        let index = self.leaf.vars.iter().position(|plan| plan.key() == name)?;
        self.values.get(index).copied()
    }
}

impl<'p, T> RawMatch<'p, T> {
    /// The value attached when the route was registered.
    #[inline]
    #[must_use]
    pub fn value(&self) -> &'p T {
        self.value
    }

    /// Iterates the captured `(name, value)` pairs of this match.
    ///
    /// Pairs follow template declaration order.
    #[inline]
    pub fn captures(&self) -> impl Iterator<Item = (&'p str, &'p str)> + '_ {
        self.leaf.vars.iter().map(VarPlan::key).zip(self.values.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use alloc::{format, vec};

    use http_path_template::{Grammar, PathTemplate};
    use routerama_build::Route;

    use super::*;
    use crate::raw_resolver::RawResolver;

    #[test]
    fn captures_yields_every_name_value_pair_in_order() {
        let router = RawResolver::new([Route::new(
            "Review",
            "GET",
            PathTemplate::parse("/books/{book}/reviews/{review}", Grammar::default()).expect("valid template"),
        )]);
        let matched = router.resolve("GET", "/books/rust/reviews/42").expect("match");
        let pairs: Vec<(&str, &str)> = matched.captures().collect();
        assert_eq!(pairs, vec![("book", "rust"), ("review", "42")]);
    }

    #[test]
    fn dyn_match_is_cloneable_and_debuggable() {
        // The payload deliberately does not implement `Debug`.
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                "GET",
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let resolver = RawResolver::with_values([(mk("GetBook", "/books/{book}"), || {})]);

        let matched = resolver.resolve("GET", "/books/rust").expect("match");
        let cloned = matched.clone();
        assert_eq!(cloned.name(), "GetBook");
        assert_eq!(cloned.capture("book"), Some("rust"));

        let debug = format!("{matched:?}");
        assert!(debug.contains("RawMatch"), "{debug}");
        assert!(debug.contains("GetBook"), "{debug}");
        assert!(debug.contains("book"), "{debug}");
    }
}
