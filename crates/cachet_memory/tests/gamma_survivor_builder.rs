// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder diagnostics preserve configuration without running the cache clock.

use cachet_memory::InMemoryCacheBuilder;

#[test]
fn gamma_survivor_builder_debug_preserves_default_capacity_policy_and_hasher_labels() {
    let builder = InMemoryCacheBuilder::<u32, u32>::new();
    let output = format!("{builder:?}");
    assert!(output.contains("initial_capacity: None"), "{output}");
    assert!(output.contains("eviction_policy: EvictionPolicy::TinyLfu"), "{output}");
    assert!(output.contains("hasher:"), "{output}");
}
