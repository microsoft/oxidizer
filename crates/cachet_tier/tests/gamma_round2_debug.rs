// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mock-cache diagnostic field names.

#![cfg(feature = "test-util")]

#[test]
fn gamma_round2_mock_cache_debug_labels_its_data() {
    let cache = cachet_tier::MockCache::<u32, u32>::new();
    let debug = format!("{cache:?}");
    assert!(debug.starts_with("MockCache { data: "), "{debug}");
    assert!(debug.contains("operations: "), "{debug}");
    assert!(debug.ends_with("fail_when: false }"), "{debug}");
}
