// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared resources and memory access.

use bytesbuf::mem::GlobalPool;
use compressors::Resources;

#[test]
fn resources_expose_one_global_instance_and_usable_memory() {
    assert!(
        std::ptr::eq(Resources::global(), Resources::global()),
        "every caller must see the same global resources"
    );

    let resources = Resources::new(GlobalPool::new());
    assert!(
        resources.memory().reserve(16).remaining_capacity() >= 16,
        "the memory provider must be reachable"
    );
    assert!(format!("{resources:?}").contains("Resources"));
}
