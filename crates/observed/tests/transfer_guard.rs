// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contracts for the thread-bound transfer guard.

use std::panic::{RefUnwindSafe, UnwindSafe};

use observed::Sink;
use observed::context::TransferGuard;
use observed::enrichment::{EnrichFnExt as _, Enrichment, EnrichmentEntry};
use static_assertions::assert_not_impl_any;

assert_not_impl_any!(TransferGuard: Send, Sync);
// Restored enrichment can contain user trait objects that are not unwind-safe.
assert_not_impl_any!(TransferGuard: UnwindSafe, RefUnwindSafe);

/// Supplies distinct markers that reveal disclosure through `Debug`.
struct Secret(&'static str);

impl Enrichment for Secret {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        vec![EnrichmentEntry::unclassified(self.0, 1)]
    }
}

#[test]
fn transfer_guard_debug_does_not_disclose_context() {
    let sink = Sink::builder("transfer_guard").build();
    let transfer = (|| sink.transfer_context()).enrich(&sink, Secret("captured-secret"))();

    (|| {
        let guard = transfer.apply_current_thread();
        let debug = format!("{guard:?}");
        assert!(!debug.contains("prior-secret"));
        assert!(!debug.contains("captured-secret"));
    })
    .enrich(&sink, Secret("prior-secret"))();
}
