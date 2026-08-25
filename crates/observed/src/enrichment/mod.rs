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
pub(crate) use slot::{EnrichmentNode, EnrichmentTransfer, Guard, OptEnrichmentNode, Slot};
