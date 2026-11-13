// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `DynamicCache`.

use cachet_tier::DynamicCache;
#[cfg(feature = "test-util")]
use cachet_tier::MockCache;

#[cfg(feature = "test-util")]
#[test]
#[cfg_attr(miri, ignore = "Debug snapshots run natively and under cargo careful")]
fn debug_output_is_correct() {
    let cache = DynamicCache::new(MockCache::<String, i32>::new());
    insta::assert_debug_snapshot!(cache);
}
