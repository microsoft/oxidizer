// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `DynamicCache`.

#[cfg(feature = "test-util")]
use cachet_tier::{DynamicCacheExt, MockCache};

#[cfg(feature = "test-util")]
#[test]
fn debug_output_is_correct() {
    let cache = MockCache::<String, i32>::new().into_dynamic();
    let debug_output = format!("{cache:?}");
    assert_eq!(debug_output, "DynamicCache");
}
