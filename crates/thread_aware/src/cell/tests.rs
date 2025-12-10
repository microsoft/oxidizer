// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{
    self,
    atomic::{AtomicI32, Ordering},
};

use crate::closure::relocate;
use crate::{MemoryAffinity, PinnedAffinity, ThreadAware, Unaware};

// We don't use PerCore here because we want to test the raw Trc itself.
type PerCore<T> = crate::Arc<T, crate::PerCore>;

#[derive(Clone, Debug)]
struct Counter {
    value: sync::Arc<AtomicI32>,
}

impl Counter {
    fn new() -> Self {
        Self {
            value: sync::Arc::new(AtomicI32::new(0)),
        }
    }
    fn increment_by(&self, v: i32) {
        self.value.fetch_add(v, Ordering::AcqRel);
    }
    fn value(&self) -> i32 {
        self.value.load(Ordering::Acquire)
    }
}

impl ThreadAware for Counter {
    fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
        Self {
            value: sync::Arc::new(AtomicI32::new(0)),
        }
    }
}

#[test]
#[cfg(feature = "test-util")]
fn transfer_creates_new_value() {
    use crate::test_util::create_manual_pinned_affinities;
    let affinities = create_manual_pinned_affinities(&[2]);
    let source = affinities[0].into();
    let destination = affinities[1];

    let pmr = PerCore::new(Counter::new);
    pmr.increment_by(10);
    let pmr2 = pmr.clone().relocated(source, destination);
    assert_eq!(pmr.value(), 10);
    assert_eq!(pmr2.value(), 0);
}

#[test]
fn new_with_works() {
    let pmr = PerCore::new_with((), |()| Counter::new());
    pmr.increment_by(3);
    assert_eq!(pmr.value(), 3);
}

#[test]
#[cfg(feature = "test-util")]
fn test_from_unaware() {
    use crate::test_util::create_manual_pinned_affinities;

    // Create a PerCore from an unaware value (a simple i32)
    // This covers line 191 (from_unaware method)
    let per_core = PerCore::from_unaware(42);
    assert_eq!(*per_core, 42);

    // Verify it can be relocated
    let affinities = create_manual_pinned_affinities(&[2]);
    let relocated = per_core.relocated(affinities[0].into(), affinities[1]);
    assert_eq!(*relocated, 42);
}

#[test]
fn test_partialeq() {
    let value1 = PerCore::with_value(42);
    let value2 = PerCore::with_value(42);
    let value3 = PerCore::with_value(43);

    assert_eq!(value1, value2);
    assert_ne!(value1, value3);
}

#[test]
fn test_hash() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let value1 = PerCore::with_value(42);
    let value2 = PerCore::with_value(42);
    let value3 = PerCore::with_value(43);

    let mut hasher1 = DefaultHasher::new();
    value1.hash(&mut hasher1);
    let hash1 = hasher1.finish();

    let mut hasher2 = DefaultHasher::new();
    value2.hash(&mut hasher2);
    let hash2 = hasher2.finish();

    let mut hasher3 = DefaultHasher::new();
    value3.hash(&mut hasher3);
    let hash3 = hasher3.finish();

    assert_eq!(hash1, hash2);
    assert_ne!(hash1, hash3);
}

#[test]
fn test_partialord() {
    let value1 = PerCore::with_value(42);
    let value2 = PerCore::with_value(43);

    assert!(value1 < value2);
    assert!(value2 > value1);
}

#[test]
fn test_ord() {
    let value1 = PerCore::with_value(42);
    let value2 = PerCore::with_value(43);
    let value3 = PerCore::with_value(42);

    assert_eq!(value1.cmp(&value2), std::cmp::Ordering::Less);
    assert_eq!(value2.cmp(&value1), std::cmp::Ordering::Greater);
    assert_eq!(value1.cmp(&value3), std::cmp::Ordering::Equal);
}

#[test]
fn test_trc_clone() {
    let value = PerCore::with_value(42);
    let cloned_value = value.clone();
    assert_eq!(*value, *cloned_value);
}

#[test]
fn test_into_arc() {
    let trc = PerCore::with_closure(relocate((), |()| 42));
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(42);
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(Unaware(42));
    let _arc = trc.into_arc();
}

#[test]
fn test_from() {
    let trc = PerCore::with_closure(relocate((), |()| 42));
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(42);
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(Unaware(42));
    let _arc = trc.into_arc().into_arc();
}

#[test]
#[cfg(feature = "test-util")]
fn test_trc_relocated_with_factory_data() {
    use crate::test_util::create_manual_pinned_affinities;

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create a Trc with a value that implements ThreadAware + Clone
    // This will use Factory::Data
    let trc_affinity1 = PerCore::with_value(42);
    assert_eq!(*trc_affinity1, 42);

    // Relocate to another affinity, which should trigger Factory::Data path
    // and call data.relocated(source, destination) at line 219
    let trc_affinity2 = trc_affinity1.relocated(affinity1, affinity2);
    assert_eq!(*trc_affinity2, 42);
}

#[test]
#[cfg(feature = "test-util")]
fn test_trc_relocated_reuses_existing_value() {
    use crate::test_util::create_manual_pinned_affinities;

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create a Trc and clone it before relocating
    let trc1 = PerCore::with_value(42);
    let trc2 = trc1.clone();

    // Relocate the first Trc to affinity2
    // This creates a new value in the destination storage
    let trc1_relocated = trc1.relocated(affinity1, affinity2);
    assert_eq!(*trc1_relocated, 42);

    // Relocate the cloned Trc to the same destination
    // This should hit line 428 where it finds the existing value in storage
    // and reuses it instead of creating a new one
    let trc2_relocated = trc2.relocated(affinity1, affinity2);
    assert_eq!(*trc2_relocated, 42);

    // Both relocated Trcs should point to the same sync::Arc (deduplication)
    assert!(std::sync::Arc::ptr_eq(&trc1_relocated.into_arc(), &trc2_relocated.into_arc()));
}

#[test]
#[cfg(feature = "test-util")]
fn test_from_storage() {
    use crate::test_util::create_manual_pinned_affinities;
    use std::sync::{Arc, RwLock};

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0];

    // Create a storage and populate it with a value for affinity1
    let mut storage = super::storage::Storage::new();
    let value = Arc::new(100);
    storage.replace(affinity1, Arc::clone(&value));

    let storage_arc = Arc::new(RwLock::new(storage));

    // Create a Trc from the storage at affinity1
    // This should call line 400 (from_storage method)
    let trc = PerCore::from_storage(Arc::clone(&storage_arc), affinity1);

    // Verify the value is correct
    assert_eq!(*trc, 100);

    // Verify it points to the same Arc we put in storage
    assert!(Arc::ptr_eq(&trc.into_arc(), &value));
}

#[test]
#[cfg(feature = "test-util")]
fn test_factory_clone_with_data() {
    // This test covers line 142: Self::Data(data_fn) => Self::Data(*data_fn)
    // We create a Trc with Factory::Data, clone it, and verify the factory is properly cloned
    use crate::test_util::create_manual_pinned_affinities;

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create a Trc with a value that uses Factory::Data (ThreadAware + Clone)
    let trc1 = PerCore::with_value(42);

    // Clone the Trc - this should exercise line 142 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 42);
    assert_eq!(*trc2, 42);

    // Relocate both to verify the cloned factory works properly
    let trc1_relocated = trc1.relocated(affinity1, affinity2);
    let trc2_relocated = trc2.relocated(affinity1, affinity2);

    assert_eq!(*trc1_relocated, 42);
    assert_eq!(*trc2_relocated, 42);
}

#[test]
#[cfg(feature = "test-util")]
fn test_factory_clone_with_closure() {
    // This test covers line 141: Self::Closure(closure, closure_source) => Self::Closure(sync::Arc::clone(closure), *closure_source)
    // We create a Trc with Factory::Closure via with_closure, clone it, and verify the factory is properly cloned
    use crate::test_util::create_manual_pinned_affinities;

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create a Trc with a closure that uses Factory::Closure
    let trc1 = PerCore::with_closure(relocate((), |()| 100));

    // Clone the Trc - this should exercise line 141 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 100);
    assert_eq!(*trc2, 100);

    // Relocate both to verify the cloned factory (closure) works properly
    let trc1_relocated = trc1.relocated(affinity1, affinity2);
    let trc2_relocated = trc2.relocated(affinity1, affinity2);

    assert_eq!(*trc1_relocated, 100);
    assert_eq!(*trc2_relocated, 100);

    // Both relocated Trcs should point to the same sync::Arc due to deduplication
    assert!(std::sync::Arc::ptr_eq(&trc1_relocated.into_arc(), &trc2_relocated.into_arc()));
}

#[test]
#[cfg(feature = "test-util")]
fn test_factory_clone_with_manual() {
    // This test covers line 143: Self::Manual => Self::Manual
    // We create a Trc from storage (Factory::Manual), clone it, and verify the factory is properly cloned
    use crate::test_util::create_manual_pinned_affinities;
    use std::sync::{Arc, RwLock};

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0];

    // Create a storage and populate it with a value for affinity1
    let mut storage = super::storage::Storage::new();
    let value = Arc::new(200);
    storage.replace(affinity1, Arc::clone(&value));

    let storage_arc = Arc::new(RwLock::new(storage));

    // Create a Trc from storage - this uses Factory::Manual
    let trc1 = PerCore::from_storage(Arc::clone(&storage_arc), affinity1);

    // Clone the Trc - this should exercise line 143 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 200);
    assert_eq!(*trc2, 200);

    // Both should point to the same Arc
    assert!(Arc::ptr_eq(&trc1.into_arc(), &trc2.into_arc()));
}

#[test]
#[cfg(feature = "test-util")]
fn test_factory_manual_relocated() {
    // This test covers line 453: Factory::Manual branch in relocated()
    // When a Trc is created from storage (Factory::Manual) and relocated to a new affinity,
    // it should behave like sync::Arc<T> and just clone the value without creating new data

    use crate::test_util::create_manual_pinned_affinities;
    use std::sync::{Arc, RwLock};

    let affinities = create_manual_pinned_affinities(&[2]);
    let affinity1 = affinities[0];
    let affinity2 = affinities[1];

    // Create a storage with a value at affinity1
    let mut storage = super::storage::Storage::new();
    let value = Arc::new(100);
    storage.replace(affinity1, Arc::clone(&value));

    let storage_arc = Arc::new(RwLock::new(storage));

    // Create a Trc from storage - this uses Factory::Manual
    let trc = PerCore::from_storage(Arc::clone(&storage_arc), affinity1);
    assert_eq!(*trc, 100);

    // Relocate to affinity2 where no data exists
    // This should trigger line 453 (Factory::Manual branch)
    // and behave like Arc<T> by just cloning the reference
    let trc_relocated = trc.relocated(affinity1.into(), affinity2);

    // The value should still be 100
    assert_eq!(*trc_relocated, 100);

    // The relocated Trc should point to the same Arc as the original
    // because Factory::Manual just clones the Arc
    assert!(Arc::ptr_eq(&trc_relocated.into_arc(), &value));
}

#[test]
#[cfg(feature = "test-util")]
fn test_relocated_unknown_source() {
    use crate::test_util::create_manual_pinned_affinities;
    use crate::{MemoryAffinity, ThreadAware};

    let affinities = create_manual_pinned_affinities(&[2]);

    let source = MemoryAffinity::Unknown;
    let destination = affinities[1];

    let trc = PerCore::with_value(42);

    let relocated_trc = trc.relocated(source, destination);
    assert_eq!(*relocated_trc, 42);
}
