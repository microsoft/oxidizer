// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{self};

use crate::affinity::{MemoryAffinity, PinnedAffinity, pinned_affinities};
use crate::closure::relocate;
use crate::{ThreadAware, Unaware};

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
fn transfer_creates_new_value() {
    let affinities = pinned_affinities(&[2]);
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
fn test_from_unaware() {
    // Create a PerCore from an unaware value (a simple i32)
    // This covers line 191 (from_unaware method)
    let per_core = PerCore::from_unaware(42);
    assert_eq!(*per_core, 42);

    // Verify it can be relocated
    let affinities = pinned_affinities(&[2]);
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
fn test_trc_relocated_with_factory_data() {
    let affinities = pinned_affinities(&[2]);
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
fn test_trc_relocated_reuses_existing_value() {
    let affinities = pinned_affinities(&[2]);
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
fn test_from_storage() {
    use std::sync::{Arc, RwLock};

    let affinities = pinned_affinities(&[2]);
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
fn test_factory_clone_with_data() {
    // This test covers line 142: Self::Data(data_fn) => Self::Data(*data_fn)
    // We create a Trc with Factory::Data, clone it, and verify the factory is properly cloned
    let affinities = pinned_affinities(&[2]);
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
fn test_factory_clone_with_closure() {
    // This test covers line 141: Self::Closure(closure, closure_source) => Self::Closure(sync::Arc::clone(closure), *closure_source)
    // We create a Trc with Factory::Closure via with_closure, clone it, and verify the factory is properly cloned
    let affinities = pinned_affinities(&[2]);
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
fn test_factory_clone_with_manual() {
    // This test covers line 143: Self::Manual => Self::Manual
    // We create a Trc from storage (Factory::Manual), clone it, and verify the factory is properly cloned
    use std::sync::{Arc, RwLock};

    let affinities = pinned_affinities(&[2]);
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
fn test_factory_manual_relocated() {
    // This test covers line 453: Factory::Manual branch in relocated()
    // When a Trc is created from storage (Factory::Manual) and relocated to a new affinity,
    // it should behave like sync::Arc<T> and just clone the value without creating new data
    use std::sync::{Arc, RwLock};

    let affinities = pinned_affinities(&[2]);
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
fn test_relocated_unknown_source() {
    let affinities = pinned_affinities(&[2]);

    let source = MemoryAffinity::Unknown;
    let destination = affinities[1];

    let trc = PerCore::with_value(42);

    let relocated_trc = trc.relocated(source, destination);
    assert_eq!(*relocated_trc, 42);
}

#[test]
fn test_strong_count() {
    // Test strong_count with a single reference
    let arc = PerCore::new(Counter::new);
    assert_eq!(PerCore::strong_count(&arc), 1);

    // Test strong_count with multiple references
    let arc2 = arc.clone();
    assert_eq!(PerCore::strong_count(&arc), 2);
    assert_eq!(PerCore::strong_count(&arc2), 2);

    let arc3 = arc.clone();
    assert_eq!(PerCore::strong_count(&arc), 3);
    assert_eq!(PerCore::strong_count(&arc2), 3);
    assert_eq!(PerCore::strong_count(&arc3), 3);

    // Test strong_count after dropping a reference
    drop(arc2);
    assert_eq!(PerCore::strong_count(&arc), 2);
    assert_eq!(PerCore::strong_count(&arc3), 2);

    drop(arc3);
    assert_eq!(PerCore::strong_count(&arc), 1);
}

#[test]
fn test_strong_count_after_relocation() {
    let affinities = pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create an Arc with multiple strong references
    let arc1 = PerCore::new(Counter::new);
    let arc2 = arc1.clone();
    assert_eq!(PerCore::strong_count(&arc1), 2);

    // Relocate one of them
    let arc1_relocated = arc1.relocated(affinity1, affinity2);

    // After relocation:
    // - arc1_relocated holds a reference to a new Arc created for affinity2
    // - The storage at affinity2 also holds a reference, but strong_count excludes internal refs
    // - Therefore, strong_count for arc1_relocated is 1
    assert_eq!(PerCore::strong_count(&arc1_relocated), 1);

    // arc2 refers to the original Arc at affinity1
    // - arc2 itself holds a reference
    // - The storage at affinity1 also holds a reference, but strong_count excludes internal refs
    // - Therefore, strong_count for arc2 is 1
    assert_eq!(PerCore::strong_count(&arc2), 1);
}

#[test]
fn test_strong_count_with_deduplication() {
    let affinities = pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create an Arc and clone it
    let arc1 = PerCore::new(Counter::new);
    let arc2 = arc1.clone();

    // Relocate both to the same destination
    // They should share the same underlying Arc in the destination
    let arc1_relocated = arc1.relocated(affinity1, affinity2);
    let arc2_relocated = arc2.relocated(affinity1, affinity2);

    // Both should point to the same underlying Arc (deduplication)
    // The strong count includes:
    // - arc1_relocated (1)
    // - arc2_relocated (1)
    // Storage reference at affinity2 is excluded by strong_count
    assert_eq!(PerCore::strong_count(&arc1_relocated), 2);
    assert_eq!(PerCore::strong_count(&arc2_relocated), 2);
}

#[test]
fn test_strong_count_independent_across_affinities() {
    let affinities = pinned_affinities(&[2]);
    let affinity1 = affinities[0].into();
    let affinity2 = affinities[1];

    // Create an Arc on affinity1 with strong_count = 1
    let arc_a = PerCore::new(Counter::new);
    assert_eq!(PerCore::strong_count(&arc_a), 1);

    // Relocate to affinity2, creating a separate instance there
    let arc_b = arc_a.clone().relocated(affinity1, affinity2);
    assert_eq!(PerCore::strong_count(&arc_b), 1); // arc_b only; storage ref excluded

    // Clone arc_a on affinity1 - this should NOT affect arc_b on affinity2
    let arc_a2 = arc_a.clone();
    // arc_a is now referenced by:
    // - arc_a itself
    // - arc_a2
    // Storage at affinity1 also holds a reference, but strong_count excludes internal refs
    assert_eq!(PerCore::strong_count(&arc_a), 2);
    assert_eq!(PerCore::strong_count(&arc_a2), 2);
    // arc_b on affinity2 is unaffected by the clone on affinity1
    assert_eq!(PerCore::strong_count(&arc_b), 1); // Still 1; unaffected by clone on affinity1
}

#[test]
fn test_relocated_source_equals_destination_first_call_reuses_eager_value() {
    // Regression test for two coupled invariants of `Arc::relocated` when source == destination:
    //
    //   1. First-call dedup: the eager value built inside `with_closure` for the calling
    //      thread's affinity must be reused. Calling the factory again would violate the
    //      "factory runs at most once per affinity" contract that callers like
    //      `Instantiation::All` rely on, producing the off-by-one factory-call count that
    //      caused the original flake.
    //
    //   2. No storage corruption: after the relocate completes, the storage slot for that
    //      affinity must hold the value that was actually returned, not a stale pre-relocation
    //      value written by the source-restore branch.
    let affinities = pinned_affinities(&[2]);
    let affinity = affinities[0];

    // `Counter`'s inner atomic is shared across clones, so mutating it here lets us tell
    // "reused eager value" (sees 42) apart from "fresh factory build" (Counter::new ⇒ 0).
    let arc = PerCore::new(Counter::new);
    arc.increment_by(42);
    assert_eq!(arc.value(), 42);

    // First relocation with source == destination must reuse the eager value, NOT rerun
    // the factory. This is the fix for the `counts > len` flake.
    let relocated = arc.relocated(affinity.into(), affinity);
    assert_eq!(
        relocated.value(),
        42,
        "first relocated(source, source) must reuse the eager value from with_closure"
    );

    // Subsequent relocations from the same slot must read from storage (cached), not
    // resurrect a stale pre-relocation value via the source-restore branch.
    let relocated_again = relocated.relocated(affinity.into(), affinity);
    assert_eq!(
        relocated_again.value(),
        42,
        "subsequent relocation must see the cached storage value, not a stale or fresh one"
    );
}

#[test]
fn test_with_closure_relocate_to_source_reuses_eager_value() {
    // Regression test for the flaky `Instantiation::All` factory-call count.
    //
    // `with_closure` eagerly invokes the factory once to materialize `self.value` for the
    // calling thread's affinity, but does not record which affinity that was. The first
    // `relocated()` call observes the caller's affinity as `source`, so when source equals
    // the destination, the eager value must be reused and the factory must NOT be called
    // again. Without this shortcut, parallel relocations to all worker affinities — where
    // one task targets the source affinity itself — race on the storage write lock and
    // the factory-call count becomes non-deterministic.
    let affinities = pinned_affinities(&[2]);
    let affinity_a = affinities[0];
    let affinity_b = affinities[1];

    let trc = PerCore::with_closure(relocate((), |()| 42));

    // Capture the eagerly-built inner Arc so we can compare identity later.
    let pre_arc = trc.clone().into_arc();
    assert_eq!(*pre_arc, 42, "with_closure should eagerly build the value");

    // First relocation has source == destination. With the fix, this must reuse the eager
    // value rather than calling the factory again — verified via Arc::ptr_eq on the inner
    // sync::Arc<T>.
    let trc_a = trc.clone().relocated(affinity_a.into(), affinity_a);
    let post_arc = trc_a.into_arc();
    assert!(
        sync::Arc::ptr_eq(&pre_arc, &post_arc),
        "relocated(source, source) must reuse the eager sync::Arc from with_closure"
    );

    // Relocating to a different affinity still requires a fresh per-affinity value, so
    // the inner Arc must differ (factory was invoked for the new affinity).
    let trc_b = trc.relocated(affinity_a.into(), affinity_b);
    let other_arc = trc_b.into_arc();
    assert!(
        !sync::Arc::ptr_eq(&pre_arc, &other_arc),
        "relocated(source, other) must build a fresh per-affinity value"
    );
    assert_eq!(*other_arc, 42);
}
