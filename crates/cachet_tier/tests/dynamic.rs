// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `DynamicCache`.

use cachet_tier::DynamicCache;
#[cfg(feature = "test-util")]
use cachet_tier::MockCache;

#[cfg(feature = "test-util")]
#[test]
fn debug_output_is_correct() {
    let cache = DynamicCache::new(MockCache::<String, i32>::new());
    let debug_output = format!("{cache:?}");
    assert_eq!(debug_output, "DynamicCache");
}
