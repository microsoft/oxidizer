// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration coverage for the unified allocator declaration.

use rallocator::config::{SizeClassLayout, Tunables};

rallocator::rallocator! {
    size_classes: [16, 32, 64, 16_384],
    partial_slab_scan_limit: 8,
    recycled_bitmap_batch_max_block_size: 512,
    medium_purge_delay_ms: 2_000,
}

#[test]
fn one_macro_configures_allocator_and_tunables() {
    const {
        assert!(__RallocatorMacroTunables::PARTIAL_SLAB_SCAN_LIMIT == 8);
        assert!(__RallocatorMacroTunables::RECYCLED_BITMAP_BATCH_MAX_BLOCK_SIZE == 512);
        assert!(__RallocatorMacroTunables::MEDIUM_PURGE_DELAY_MS == 2_000);
        assert!(<__RallocatorInlineSizeClasses as SizeClassLayout>::SIZES.len() == 4);
    }

    let value = Box::new(42_u64);
    assert_eq!(*value, 42);
}
