// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{self};

use crate::affinity::{Affinity, pinned_affinities};
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
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
        self.value = sync::Arc::new(AtomicI32::new(0));
    }
}

#[test]
fn transfer_creates_new_value() {
    let affinities = pinned_affinities(&[2]);
    let source = Some(affinities[0]);
    let destination = affinities[1];

    let pmr = PerCore::new(Counter::new);
    pmr.increment_by(10);
    let mut pmr2 = pmr.clone();
    pmr2.relocate(source, destination);
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
fn new_with_relocate_forwards_to_data() {
    // Exercises BoxedRelocate::relocate and the Factory::Closure path.
    // Uses a data value whose relocate changes observable state, so we can
    // verify that the closure's data was actually relocated before call_once.
    #[derive(Clone)]
    struct Seed(bool);

    impl ThreadAware for Seed {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.0 = true;
        }
    }

    let affinities = pinned_affinities(&[2]);
    let source = Some(affinities[0]);
    let destination = affinities[1];

    let pmr = PerCore::new_with(Seed(false), |seed| {
        let c = Counter::new();
        // The factory output depends on whether the seed was relocated.
        if seed.0 {
            c.increment_by(999);
        }
        c
    });
    assert_eq!(pmr.value(), 0, "initial factory should see un-relocated seed");

    let mut relocated = pmr;
    relocated.relocate(source, destination);
    assert_eq!(
        relocated.value(),
        999,
        "factory must see relocated seed (BoxedRelocate must forward relocate)"
    );
}

#[test]
fn test_from_unaware() {
    // Create a PerCore from an unaware value (a simple i32)
    // This covers line 191 (from_unaware method)
    let per_core = PerCore::from_unaware(42);
    assert_eq!(*per_core, 42);

    // Verify it can be relocated
    let affinities = pinned_affinities(&[2]);
    let mut relocated = per_core;
    relocated.relocate(Some(affinities[0]), affinities[1]);
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
    let trc = PerCore::new(|| 42);
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(42);
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(Unaware(42));
    let _arc = trc.into_arc();
}

#[test]
fn test_from() {
    let trc = PerCore::new(|| 42);
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(42);
    let _arc = trc.into_arc();

    let trc = PerCore::with_value(Unaware(42));
    let _arc = trc.into_arc().into_arc();
}

#[test]
fn test_trc_relocated_with_factory_data() {
    let affinities = pinned_affinities(&[2]);
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create a Trc with a value that implements ThreadAware + Clone
    // This will use Factory::Data
    let trc_affinity1 = PerCore::with_value(42);
    assert_eq!(*trc_affinity1, 42);

    // Relocate to another affinity, which should trigger Factory::Data path
    // and call data.relocate(source, destination) at line 219
    let mut trc_affinity2 = trc_affinity1;
    trc_affinity2.relocate(affinity1, affinity2);
    assert_eq!(*trc_affinity2, 42);
}

#[test]
fn test_trc_relocated_reuses_existing_value() {
    let affinities = pinned_affinities(&[2]);
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create a Trc and clone it before relocating
    let trc1 = PerCore::with_value(42);
    let trc2 = trc1.clone();

    // Relocate the first Trc to affinity2
    // This creates a new value in the destination storage
    let mut trc1_relocated = trc1;
    trc1_relocated.relocate(affinity1, affinity2);
    assert_eq!(*trc1_relocated, 42);

    // Relocate the cloned Trc to the same destination
    // This should hit line 428 where it finds the existing value in storage
    // and reuses it instead of creating a new one
    let mut trc2_relocated = trc2;
    trc2_relocated.relocate(affinity1, affinity2);
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
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create a Trc with a value that uses Factory::Data (ThreadAware + Clone)
    let trc1 = PerCore::with_value(42);

    // Clone the Trc - this should exercise line 142 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 42);
    assert_eq!(*trc2, 42);

    // Relocate both to verify the cloned factory works properly
    let mut trc1_relocated = trc1;
    trc1_relocated.relocate(affinity1, affinity2);
    let mut trc2_relocated = trc2;
    trc2_relocated.relocate(affinity1, affinity2);

    assert_eq!(*trc1_relocated, 42);
    assert_eq!(*trc2_relocated, 42);
}

#[test]
fn test_factory_clone_with_closure_boxed() {
    // This test covers line 141: Self::Closure(closure, closure_source) => Self::Closure(sync::Arc::clone(closure), *closure_source)
    // We create a Trc with Factory::Closure via with_closure, clone it, and verify the factory is properly cloned
    let affinities = pinned_affinities(&[2]);
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create a Trc with a closure that uses Factory::Closure
    let trc1 = PerCore::new(|| 100);

    // Clone the Trc - this should exercise line 141 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 100);
    assert_eq!(*trc2, 100);

    // Relocate both to verify the cloned factory (closure) works properly
    let mut trc1_relocated = trc1;
    trc1_relocated.relocate(affinity1, affinity2);
    let mut trc2_relocated = trc2;
    trc2_relocated.relocate(affinity1, affinity2);

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
    let mut trc_relocated = trc;
    trc_relocated.relocate(Some(affinity1), affinity2);

    // The value should still be 100
    assert_eq!(*trc_relocated, 100);

    // The relocated Trc should point to the same Arc as the original
    // because Factory::Manual just clones the Arc
    assert!(Arc::ptr_eq(&trc_relocated.into_arc(), &value));
}

#[test]
fn test_relocated_unknown_source() {
    let affinities = pinned_affinities(&[2]);

    let source = None;
    let destination = affinities[1];

    let trc = PerCore::with_value(42);

    let mut relocated_trc = trc;
    relocated_trc.relocate(source, destination);
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
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create an Arc with multiple strong references
    let arc1 = PerCore::new(Counter::new);
    let arc2 = arc1.clone();
    assert_eq!(PerCore::strong_count(&arc1), 2);

    // Relocate one of them
    let mut arc1_relocated = arc1;
    arc1_relocated.relocate(affinity1, affinity2);

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
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create an Arc and clone it
    let arc1 = PerCore::new(Counter::new);
    let arc2 = arc1.clone();

    // Relocate both to the same destination
    // They should share the same underlying Arc in the destination
    let mut arc1_relocated = arc1;
    arc1_relocated.relocate(affinity1, affinity2);
    let mut arc2_relocated = arc2;
    arc2_relocated.relocate(affinity1, affinity2);

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
    let affinity1 = Some(affinities[0]);
    let affinity2 = affinities[1];

    // Create an Arc on affinity1 with strong_count = 1
    let arc_a = PerCore::new(Counter::new);
    assert_eq!(PerCore::strong_count(&arc_a), 1);

    // Relocate to affinity2, creating a separate instance there
    let mut arc_b = arc_a.clone();
    arc_b.relocate(affinity1, affinity2);
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
fn test_relocated_source_equals_destination_does_not_corrupt_storage() {
    // Regression test: when source == destination, Arc::relocated() must NOT overwrite the
    // newly-created value in storage with the stale pre-relocation value.
    let affinities = pinned_affinities(&[2]);
    let affinity = affinities[0];

    // Create an Arc whose Counter starts at zero, then advance it.
    let arc = PerCore::new(Counter::new);
    arc.increment_by(42);
    assert_eq!(arc.value(), 42);

    // Relocate with source == destination.  The ThreadAware impl always creates a *new*
    // Counter (value resets to 0), so `relocate` must result in 0 and must also leave
    // storage holding 0 (not the stale 42).
    let mut relocated = arc;
    relocated.relocate(Some(affinity), affinity);
    assert_eq!(relocated.value(), 0, "relocated value should come from factory");

    // A second relocation from the same slot must find the factory-created value (0) in
    // storage, not the stale pre-relocation value (42).  Before the bug fix, the first
    // relocated() call wrote the stale Arc<Counter(42)> back into the storage slot,
    // so the second call's `get_clone` fast-path would return 42 instead of 0.
    relocated.relocate(Some(affinity), affinity);
    assert_eq!(
        relocated.value(),
        0,
        "subsequent relocation must not see stale pre-relocation value from storage"
    );
}

#[test]
fn with_clone_fn_relocates_clone() {
    let affinities = pinned_affinities(&[2]);
    let source = Some(affinities[0]);
    let destination = affinities[1];

    // Counter::relocated resets value to 0, so we can detect if it was called.
    let arc = super::Arc::<Counter, crate::PerCore>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));

    arc.increment_by(42);
    assert_eq!(arc.value(), 42);

    // Relocating should clone the Counter and call relocated() on the clone,
    // which resets the value to 0.
    let mut relocated = arc;
    relocated.relocate(source, destination);
    assert_eq!(relocated.value(), 0, "relocated() must be called on the clone");
}

#[test]
fn with_clone_fn_dyn_trait_relocates_correctly() {
    // Exercises the ErasedCloneFn path with a dyn Trait object, which is the
    // primary use case. The unsafe &T -> &V cast inside CloneAdapter is
    // exercised here and validated under Miri.
    trait Plugin: ThreadAware + Send + Sync {
        fn name(&self) -> &str;
    }

    #[derive(Clone)]
    struct MyPlugin(String);

    impl Plugin for MyPlugin {
        fn name(&self) -> &str {
            &self.0
        }
    }

    impl ThreadAware for MyPlugin {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.0 = format!("{}-relocated", self.0);
        }
    }

    let affinities = pinned_affinities(&[2]);
    let source = Some(affinities[0]);
    let destination = affinities[1];

    let arc = super::Arc::<dyn Plugin, crate::PerCore>::with_clone_fn(MyPlugin("orig".into()), |p: &MyPlugin| Box::new(p.clone()));

    assert_eq!(arc.name(), "orig");

    let mut relocated = arc;
    relocated.relocate(source, destination);
    assert_eq!(relocated.name(), "orig-relocated");
}

#[test]
fn with_clone_fn_clone_and_relocate_independently() {
    // Cloning an Arc backed by ErasedCloneFn should produce independent
    // clones that can each be relocated separately.
    let affinities = pinned_affinities(&[3]);
    let source = Some(affinities[0]);
    let dest1 = affinities[1];
    let dest2 = affinities[2];

    let arc = super::Arc::<Counter, crate::PerCore>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    arc.increment_by(10);

    let mut clone1 = arc.clone();
    #[expect(clippy::redundant_clone, reason = "testing independent clones")]
    let mut clone2 = arc.clone();

    clone1.relocate(source, dest1);
    clone2.relocate(source, dest2);

    // Both should have been reset by Counter::relocate
    assert_eq!(clone1.value(), 0);
    assert_eq!(clone2.value(), 0);
}

#[test]
fn with_clone_fn_repeated_relocations() {
    // Multiple sequential relocations through the same ErasedCloneFn factory
    // must all produce correct clones.
    let affinities = pinned_affinities(&[4]);

    let arc = super::Arc::<Counter, crate::PerCore>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    arc.increment_by(99);

    let mut current = arc;
    for i in 0..3 {
        let source = Some(affinities[i]);
        let dest = affinities[i + 1];
        current.relocate(source, dest);
        // Counter resets to 0 on relocate
        assert_eq!(current.value(), 0, "relocation {i} should reset counter");
        current.increment_by(i32::try_from(i + 1).expect("loop index fits in i32"));
    }
    assert_eq!(current.value(), 3);
}

#[test]
fn with_clone_fn_debug_format() {
    // Exercises Debug formatting of the ErasedCloneFn factory path.
    let arc = super::Arc::<Counter, crate::PerCore>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    let debug = format!("{arc:?}");
    assert!(!debug.is_empty());
}

#[test]
fn with_clone_fn_deduplication_across_clones() {
    // Two clones relocated to the same destination should share the same
    // underlying value via storage deduplication.
    let affinities = pinned_affinities(&[2]);
    let source = Some(affinities[0]);
    let dest = affinities[1];

    let arc = super::Arc::<Counter, crate::PerCore>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    let clone1 = arc.clone();
    #[expect(clippy::redundant_clone, reason = "testing independent clones")]
    let clone2 = arc.clone();

    let mut r1 = clone1;
    r1.relocate(source, dest);
    let mut r2 = clone2;
    r2.relocate(source, dest);

    assert!(sync::Arc::ptr_eq(&r1.clone().into_arc(), &r2.clone().into_arc()));
}
