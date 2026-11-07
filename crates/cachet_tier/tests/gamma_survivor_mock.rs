// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Failed mock operations remain observable without changing stored state.

#![cfg(feature = "test-util")]

use std::collections::HashMap;

use cachet_tier::{CacheEntry, CacheOp, CacheTier, Error, MockCache};

#[tokio::test]
async fn gamma_survivor_failed_operations_are_recorded_and_do_not_mutate_data() {
    let cache = MockCache::with_data(HashMap::from([(7, CacheEntry::new(13))]));
    cache.fail_when(|_| true);
    assert!(format!("{cache:?}").contains("fail_when: true"));
    assert!(format!("{cache:?}").contains("data:"));
    assert!(format!("{cache:?}").contains("operations:"));
    assert!(cache.get(&7).await.unwrap_err().to_string().contains("mock: get failed"));
    assert!(
        cache
            .insert(9, CacheEntry::new(19))
            .await
            .unwrap_err()
            .to_string()
            .contains("mock: insert failed")
    );
    assert!(
        cache
            .invalidate(&7)
            .await
            .unwrap_err()
            .to_string()
            .contains("mock: invalidate failed")
    );
    assert!(cache.clear().await.unwrap_err().to_string().contains("mock: clear failed"));
    assert_eq!(
        cache.operations(),
        vec![
            CacheOp::Get(7),
            CacheOp::Insert {
                key: 9,
                entry: CacheEntry::new(19)
            },
            CacheOp::Invalidate(7),
            CacheOp::Clear
        ]
    );
    assert_eq!(cache.entry_count(), 1);
    assert!(cache.contains_key(&7));
    assert!(!cache.contains_key(&9));
    cache.clear_failures();
    cache.clear_operations();
    assert!(format!("{cache:?}").contains("fail_when: false"));
    cache.invalidate(&7).await.unwrap();
    cache.clear().await.unwrap();
    assert_eq!(cache.operations(), vec![CacheOp::Invalidate(7), CacheOp::Clear]);
    assert_eq!(cache.entry_count(), 0);
}

#[test]
fn gamma_survivor_entry_mapping_returns_the_original_error() {
    let result = CacheEntry::new(17).try_map_value::<i32, _>(|value| {
        assert_eq!(value, 17);
        Err(Error::from_message("mapping failed"))
    });
    assert!(result.unwrap_err().to_string().contains("mapping failed"));
}
