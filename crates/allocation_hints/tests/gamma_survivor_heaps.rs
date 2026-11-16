// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests heap options and identities.

use allocation_hints::heaps::{Heap, Kind, general};

#[test]
fn gamma_survivor_general_heap_preserves_nondefault_options_and_unique_ids() {
    let options = general::Options::new()
        .with_locality_segment_bytes(65536)
        .with_medium_cache_max_bytes(0);
    let first = Heap::general(options);
    let second = Heap::general(options);
    assert_eq!(first.kind(), Kind::General(options));
    assert_eq!(second.kind(), Kind::General(options));
    assert_ne!(first.id(), second.id());
    assert_eq!(second.id().get(), first.id().get() + 1);
    let cloned = first.clone();
    assert_eq!(cloned.id(), first.id());
    assert_eq!(cloned.kind(), Kind::General(options));
}
