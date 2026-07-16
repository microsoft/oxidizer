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
/// The derive macro also generates an [`IntoIterator`] implementation, so the
/// struct can be passed directly to [`.enrich()`](crate::enrichment::EnrichFutureExt::enrich).
///
/// See the [`Enrichment` derive macro](crate::Enrichment) for field attributes
/// and usage examples.
pub trait Enrichment {
    /// Converts this enrichment struct into a `Vec` of [`EnrichmentEntry`] items.
    // TODO: review this API
    fn into_entries(self) -> Vec<EnrichmentEntry>;
}
