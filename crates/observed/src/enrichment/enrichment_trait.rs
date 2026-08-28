// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment derive trait for typed enrichment structs.

use crate::enrichment::EnrichmentEntry;

/// Trait implemented by typed enrichment structs that convert into enrichment entries.
///
/// Derive this trait via `#[derive(Enrichment)]`. Unlike [`Event`](crate::Event), enrichment
/// structs have no severity, body, or metrics - they only produce key-value
/// enrichment entries that are attached to all events in scope.
///
/// See also: [`EnrichFutureExt::enrich`](crate::enrichment::EnrichFutureExt::enrich)
/// and [`EnrichFutureExt::enrich_for`](crate::enrichment::EnrichFutureExt::enrich_for).
///
/// See the [`Enrichment` derive macro](crate::Enrichment) for field attributes
/// and usage examples.
///
/// # Manual implementation
///
/// ```
/// use observed::enrichment::{Enrichment, EnrichmentEntry};
///
/// struct RequestContext {
///     request_id: &'static str,
/// }
///
/// impl Enrichment for RequestContext {
///     fn into_entries(self) -> Vec<EnrichmentEntry> {
///         vec![EnrichmentEntry::unclassified("request.id", self.request_id)]
///     }
/// }
/// ```
pub trait Enrichment {
    /// Converts this enrichment struct into its [`EnrichmentEntry`] items.
    ///
    /// The returned order is preserved when the entries are attached to a scope.
    fn into_entries(self) -> Vec<EnrichmentEntry>;
}
