// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama_build::trie::{Leaf, VarPlan};

use crate::codegen_helpers::ScannedPath;

/// A view over a resolved match's captured path variables, passed to the
/// `#[resolver]`-generated dynamic-route extractors.
///
/// Captured values are the raw, still percent-encoded slices borrowed from the
/// request path; decoding happens per field during coercion.
pub struct Captures<'m, 'p, 's> {
    leaf: &'m Leaf,
    path: &'m ScannedPath<'p, 's>,
    order: &'m [usize],
}

impl<'m, 'p, 's> Captures<'m, 'p, 's> {
    /// Wraps a scanned match in the generated variant's field order.
    #[inline]
    #[must_use]
    pub fn new(leaf: &'m Leaf, path: &'m ScannedPath<'p, 's>, order: &'m [usize]) -> Self {
        Self { leaf, path, order }
    }

    /// The raw captured value at generated field `index`.
    #[inline]
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'p str> {
        let position = *self.order.get(index)?;
        self.leaf.vars.get(position).map(|plan| materialize(plan, self.path))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl core::fmt::Debug for Captures<'_, '_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Captures")
            .field("count", &self.leaf.vars.len())
            .finish_non_exhaustive()
    }
}

/// Slices a captured variable's value out of `body`.
#[inline]
pub(crate) fn materialize<'p>(plan: &VarPlan, path: &ScannedPath<'p, '_>) -> &'p str {
    match plan {
        VarPlan::Span { a, b, .. } => path.capture(*a, *b).expect("route capture plan references scanned segment indices"),
        VarPlan::Rest { a, .. } => path
            .rest(*a)
            .expect("route rest plan starts at or before the scanned segment count"),
        VarPlan::Affix {
            a, prefix_len, suffix_len, ..
        } => path
            .affix(*a, *prefix_len, *suffix_len)
            .expect("matched affix literals delimit a valid capture"),
    }
}
