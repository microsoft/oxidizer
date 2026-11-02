// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Removal notifications without a value listener.

use std::sync::{Arc, Mutex};

use cachet_memory::{InMemoryCache, RemovalCause};
use cachet_tier::{CacheEntry, CacheTier};

#[cfg_attr(miri, ignore)] // Moka's crossbeam-epoch dependency does not support this Miri configuration.
#[tokio::test]
async fn invalidation_notifies_a_cause_only_observer() {
    let causes = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&causes);
    let cache = InMemoryCache::<u32, u32>::builder()
        .with_removal_observer(move |cause| observed.lock().unwrap().push(cause))
        .build()
        .unwrap();
    cache.insert(1, CacheEntry::new(42)).await.unwrap();

    cache.invalidate(&1).await.unwrap();

    assert_eq!(*causes.lock().unwrap(), [RemovalCause::Explicit]);
}
