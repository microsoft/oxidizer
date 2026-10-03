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

#[test]
fn realloc_uses_actual_custom_size_class() {
    let old = std::alloc::Layout::from_size_align(48, 16).unwrap();
    let new = std::alloc::Layout::from_size_align(64, 16).unwrap();
    // SAFETY: old is a nonzero valid layout.
    let address = unsafe { std::alloc::alloc(old) };
    assert!(!address.is_null());
    // SAFETY: the allocation covers old.size() writable bytes.
    unsafe { address.write_bytes(0x7b, old.size()) };
    // SAFETY: the live block has the old layout; this binary's custom
    // personality maps both layouts to 64 bytes, unlike the standard one.
    let grown = unsafe { std::alloc::realloc(address, old, new.size()) };
    assert_eq!(grown, address);
    // SAFETY: pointer retention above also establishes non-nullness.
    assert_eq!(unsafe { *grown }, 0x7b);
    // SAFETY: successful resize exposes the entire new layout.
    unsafe { grown.write_bytes(0x5a, new.size()) };
    // SAFETY: the current allocation uses new, not old.
    let shrunk = unsafe { std::alloc::realloc(grown, new, old.size()) };
    assert_eq!(shrunk, address);
    // SAFETY: the retained live block includes this initialized byte.
    assert_eq!(unsafe { *shrunk }, 0x5a);
    // SAFETY: the successful shrink restored the old layout.
    unsafe { std::alloc::dealloc(shrunk, old) };
}
