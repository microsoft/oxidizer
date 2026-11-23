// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dynamic storage through public APIs.

#![cfg(test)]

mod dynamic {
    use std::sync::Mutex;

    use cachet_tier::{CacheEntry, CacheTier, DynamicCache, Error, InsertOutcome, SizeError};

    #[derive(Debug, Default)]
    struct Storage(Mutex<Option<(String, CacheEntry<i32>)>>);

    impl CacheTier<String, i32> for Storage {
        async fn get(&self, key: &String) -> Result<Option<CacheEntry<i32>>, Error> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .as_ref()
                .filter(|(stored, _)| stored == key)
                .map(|(_, entry)| entry.clone()))
        }

        async fn insert(&self, key: String, entry: CacheEntry<i32>) -> Result<InsertOutcome, Error> {
            *self.0.lock().unwrap() = Some((key, entry));
            Ok(InsertOutcome::Accepted)
        }

        async fn invalidate(&self, key: &String) -> Result<(), Error> {
            let mut stored = self.0.lock().unwrap();
            if stored.as_ref().is_some_and(|(stored_key, _)| stored_key == key) {
                *stored = None;
            }
            Ok(())
        }

        async fn clear(&self) -> Result<(), Error> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }

        async fn len(&self) -> Result<u64, SizeError> {
            Ok(u64::from(self.0.lock().unwrap().is_some()))
        }
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn clone_shares_state() {
        let cache = Storage::default();
        let dynamic = DynamicCache::new(cache);
        let clone = dynamic.clone();

        let outcome = dynamic.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        assert_eq!(outcome, InsertOutcome::Accepted);

        let entry = clone.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);
    }
}
