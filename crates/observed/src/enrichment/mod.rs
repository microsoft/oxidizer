// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment types.

mod enrich_ext;
mod enrichment_trait;
mod entry;
mod slot;

pub use enrich_ext::{EnrichFnExt, EnrichFutureExt, Enriched};
pub use enrichment_trait::Enrichment;
#[doc(hidden)]
pub use entry::EnrichmentEntry;
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use slot::EnrichmentGuard;
pub(crate) use slot::{AppliedGuard, EnrichmentNode, EnrichmentTransfer, Guard, OptEnrichmentNode, Slot};
