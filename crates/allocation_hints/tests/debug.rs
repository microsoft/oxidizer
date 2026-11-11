// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public Debug views for allocation hints.

use allocation_hints::heaps::Heap;
use allocation_hints::with_hint_async;

#[test]
#[cfg_attr(miri, ignore = "Debug snapshots run natively and under cargo careful")]
fn async_hint_debug_snapshot() {
    let future = with_hint_async(&Heap::new(), std::future::ready(42));
    insta::with_settings!({filters => [(r"HeapId\(\s*\d+,?\s*\)", "HeapId([id])")]}, {
        insta::assert_debug_snapshot!(future, @"
        WithHint {
            heap: Heap {
                id: HeapId([id]),
                kind: General(
                    Options {
                        locality_segment_bytes: 4194304,
                        medium_cache_max_bytes: 8388608,
                    },
                ),
            },
            future: Ready(
                Some(
                    42,
                ),
            ),
        }
        ");
    });
}
