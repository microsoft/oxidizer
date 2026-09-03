// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::similar_names, reason = "relocation tests use numbered thread pairs")]

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{self};

use crate::{Thread, ThreadAware, Unaware, test_threads};

// We don't use PerThread here because we want to test the raw container itself.
type PerThreadArc<T> = crate::Arc<T, crate::PerThread>;

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
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        self.value = sync::Arc::new(AtomicI32::new(0));
    }
}

/// Records the source thread used to relocate constructor state.
#[derive(Clone)]
struct SourceRecorder(sync::Arc<sync::Mutex<Option<Thread>>>);

impl ThreadAware for SourceRecorder {
    fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
        *self.0.lock().unwrap() = source.cloned();
    }
}

#[test]
fn transfer_creates_new_value() {
    let threads = test_threads(&[2]);
    let source = Some(threads[0].clone());
    let destination = threads[1].clone();

    let pmr = PerThreadArc::new(Counter::new);
    pmr.increment_by(10);
    let mut pmr2 = pmr.clone();
    pmr2.relocate(source.as_ref(), &destination);
    assert_eq!(pmr.value(), 10);
    assert_eq!(pmr2.value(), 0);
}

#[test]
fn new_with_works() {
    let pmr = PerThreadArc::new_with((), |()| Counter::new());
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
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
            self.0 = true;
        }
    }

    let threads = test_threads(&[2]);
    let source = Some(threads[0].clone());
    let destination = threads[1].clone();

    let mut pmr = PerThreadArc::new_with(Seed(false), |seed| {
        let c = Counter::new();
        // The factory output depends on whether the seed was relocated.
        if seed.0 {
            c.increment_by(999);
        }
        c
    });
    assert_eq!(pmr.value(), 0, "initial factory should see un-relocated seed");

    pmr.relocate(source.as_ref(), &destination);
    assert_eq!(
        pmr.value(),
        999,
        "factory must see relocated seed (BoxedRelocate must forward relocate)"
    );
}

#[test]
fn test_from_unaware() {
    // Create a PerThreadArc from an unaware value (a simple i32)
    // This covers line 191 (from_unaware method)
    let mut per_thread = PerThreadArc::from_unaware(42);
    assert_eq!(*per_thread, 42);

    // Verify it can be relocated
    let threads = test_threads(&[2]);
    per_thread.relocate(Some(&threads[0]), &threads[1]);
    assert_eq!(*per_thread, 42);
}

#[test]
fn test_partialeq() {
    let value1 = PerThreadArc::with_value(42);
    let value2 = PerThreadArc::with_value(42);
    let value3 = PerThreadArc::with_value(43);

    assert_eq!(value1, value2);
    assert_ne!(value1, value3);
}

#[test]
fn test_hash() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let value1 = PerThreadArc::with_value(42);
    let value2 = PerThreadArc::with_value(42);
    let value3 = PerThreadArc::with_value(43);

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
    let value1 = PerThreadArc::with_value(42);
    let value2 = PerThreadArc::with_value(43);

    assert!(value1 < value2);
    assert!(value2 > value1);
}

#[test]
fn test_ord() {
    let value1 = PerThreadArc::with_value(42);
    let value2 = PerThreadArc::with_value(43);
    let value3 = PerThreadArc::with_value(42);

    assert_eq!(value1.cmp(&value2), std::cmp::Ordering::Less);
    assert_eq!(value2.cmp(&value1), std::cmp::Ordering::Greater);
    assert_eq!(value1.cmp(&value3), std::cmp::Ordering::Equal);
}

#[test]
fn test_trc_clone() {
    let value = PerThreadArc::with_value(42);
    let cloned_value = value.clone();
    assert_eq!(*value, *cloned_value);
}

#[test]
fn test_into_arc() {
    let trc = PerThreadArc::new(|| 42);
    let _arc = trc.into_arc();

    let trc = PerThreadArc::with_value(42);
    let _arc = trc.into_arc();

    let trc = PerThreadArc::with_value(Unaware(42));
    let _arc = trc.into_arc();
}

#[test]
fn test_from() {
    let trc = PerThreadArc::new(|| 42);
    let _arc = trc.into_arc();

    let trc = PerThreadArc::with_value(42);
    let _arc = trc.into_arc();

    let trc = PerThreadArc::with_value(Unaware(42));
    let _arc = trc.into_arc().into_arc();
}

#[test]
fn test_trc_relocated_with_factory_data() {
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create a Trc with a value that implements ThreadAware + Clone
    // This will use Factory::Data
    let trc_thread1 = PerThreadArc::with_value(42);
    assert_eq!(*trc_thread1, 42);

    // Relocate to another thread, which should trigger Factory::Data path
    // and call data.relocate(source, destination) at line 219
    let mut trc_thread2 = trc_thread1;
    trc_thread2.relocate(thread1.as_ref(), &thread2);
    assert_eq!(*trc_thread2, 42);
}

#[test]
fn test_trc_relocated_reuses_existing_value() {
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create a Trc and clone it before relocating
    let trc1 = PerThreadArc::with_value(42);
    let trc2 = trc1.clone();

    // Relocate the first Trc to thread2
    // This creates a new value in the destination storage
    let mut trc1_relocated = trc1;
    trc1_relocated.relocate(thread1.as_ref(), &thread2);
    assert_eq!(*trc1_relocated, 42);

    // Relocate the cloned Trc to the same destination
    // This should hit line 428 where it finds the existing value in storage
    // and reuses it instead of creating a new one
    let mut trc2_relocated = trc2;
    trc2_relocated.relocate(thread1.as_ref(), &thread2);
    assert_eq!(*trc2_relocated, 42);

    // Both relocated Trcs should point to the same sync::Arc (deduplication)
    assert!(std::sync::Arc::ptr_eq(&trc1_relocated.into_arc(), &trc2_relocated.into_arc()));
}

#[test]
fn test_from_storage() {
    use std::sync::Arc;

    let threads = test_threads(&[2]);
    let thread1 = threads[0].clone();

    // Create a storage and populate it with a value for thread1
    let storage = super::storage::Storage::new();
    let value = Arc::new(100);
    storage.insert(&thread1, Arc::clone(&value)).unwrap();

    let storage_arc = Arc::new(storage);

    // Create a Trc from the storage at thread1
    // This should call line 400 (from_storage method)
    let trc = PerThreadArc::from_storage(Arc::clone(&storage_arc), &thread1);

    // Verify the value is correct
    assert_eq!(*trc, 100);

    // Verify it points to the same Arc we put in storage
    assert!(Arc::ptr_eq(&trc.into_arc(), &value));
}

#[test]
fn storage_default_is_empty_then_fillable() {
    let thread = test_threads(&[2])[0].clone();

    let storage = super::storage::Storage::<i32, crate::PerThread>::default();
    assert!(storage.get(&thread).is_none());

    let value = sync::Arc::new(7);
    storage.insert(&thread, sync::Arc::clone(&value)).unwrap();
    assert!(sync::Arc::ptr_eq(&storage.get(&thread).unwrap(), &value));
}

#[test]
fn storage_insert_is_write_once() {
    // The public `Storage::insert` is write-once: the first insert into an empty key stores the
    // value and returns `Ok(())`; a second insert leaves the stored value in place and hands the
    // rejected value back as `Err`, mirroring `OnceLock::set`.
    let thread = test_threads(&[2])[0].clone();

    let storage = super::storage::Storage::<i32, crate::PerThread>::default();

    let first = sync::Arc::new(1);
    assert_eq!(storage.insert(&thread, sync::Arc::clone(&first)), Ok(()));

    let second = sync::Arc::new(2);
    let rejected = storage.insert(&thread, sync::Arc::clone(&second)).unwrap_err();
    assert!(
        sync::Arc::ptr_eq(&rejected, &second),
        "a rejected insert must hand back the exact value that was passed in"
    );
    assert!(
        sync::Arc::ptr_eq(&storage.get(&thread).unwrap(), &first),
        "the write-once key must keep the value from the first insert"
    );
}

#[test]
fn test_factory_clone_with_data() {
    // This test covers line 142: Self::Data(data_fn) => Self::Data(*data_fn)
    // We create a Trc with Factory::Data, clone it, and verify the factory is properly cloned
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create a Trc with a value that uses Factory::Data (ThreadAware + Clone)
    let trc1 = PerThreadArc::with_value(42);

    // Clone the Trc - this should exercise line 142 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 42);
    assert_eq!(*trc2, 42);

    // Relocate both to verify the cloned factory works properly
    let mut trc1_relocated = trc1;
    trc1_relocated.relocate(thread1.as_ref(), &thread2);
    let mut trc2_relocated = trc2;
    trc2_relocated.relocate(thread1.as_ref(), &thread2);

    assert_eq!(*trc1_relocated, 42);
    assert_eq!(*trc2_relocated, 42);
}

#[test]
fn test_factory_clone_with_closure_boxed() {
    // This test covers line 141: Self::Closure(closure, closure_source) => Self::Closure(sync::Arc::clone(closure), *closure_source)
    // We create a Trc with Factory::Closure via with_closure, clone it, and verify the factory is properly cloned
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create a Trc with a closure that uses Factory::Closure
    let trc1 = PerThreadArc::new(|| 100);

    // Clone the Trc - this should exercise line 141 in the Factory::clone method
    let trc2 = trc1.clone();

    // Verify both Trcs work correctly
    assert_eq!(*trc1, 100);
    assert_eq!(*trc2, 100);

    // Relocate both to verify the cloned factory (closure) works properly
    let mut trc1_relocated = trc1;
    trc1_relocated.relocate(thread1.as_ref(), &thread2);
    let mut trc2_relocated = trc2;
    trc2_relocated.relocate(thread1.as_ref(), &thread2);

    assert_eq!(*trc1_relocated, 100);
    assert_eq!(*trc2_relocated, 100);

    // Both relocated Trcs should point to the same sync::Arc due to deduplication
    assert!(std::sync::Arc::ptr_eq(&trc1_relocated.into_arc(), &trc2_relocated.into_arc()));
}

#[test]
fn test_factory_clone_with_manual() {
    // This test covers line 143: Self::Manual => Self::Manual
    // We create a Trc from storage (Factory::Manual), clone it, and verify the factory is properly cloned
    use std::sync::Arc;

    let threads = test_threads(&[2]);
    let thread1 = threads[0].clone();

    // Create a storage and populate it with a value for thread1
    let storage = super::storage::Storage::new();
    let value = Arc::new(200);
    storage.insert(&thread1, Arc::clone(&value)).unwrap();

    let storage_arc = Arc::new(storage);

    // Create a Trc from storage - this uses Factory::Manual
    let trc1 = PerThreadArc::from_storage(Arc::clone(&storage_arc), &thread1);

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
    // When a Trc is created from storage (Factory::Manual) and relocated to a new thread,
    // it should behave like sync::Arc<T> and just clone the value without creating new data
    use std::sync::Arc;

    let threads = test_threads(&[2]);
    let thread1 = threads[0].clone();
    let thread2 = threads[1].clone();

    // Create a storage with a value at thread1
    let storage = super::storage::Storage::new();
    let value = Arc::new(100);
    storage.insert(&thread1, Arc::clone(&value)).unwrap();

    let storage_arc = Arc::new(storage);

    // Create a Trc from storage - this uses Factory::Manual
    let trc = PerThreadArc::from_storage(Arc::clone(&storage_arc), &thread1);
    assert_eq!(*trc, 100);

    // Relocate to thread2 where no data exists
    // This should trigger line 453 (Factory::Manual branch)
    // and behave like Arc<T> by just cloning the reference
    let mut trc_relocated = trc;
    trc_relocated.relocate(Some(&thread1), &thread2);

    // The value should still be 100
    assert_eq!(*trc_relocated, 100);

    // The relocated Trc should point to the same Arc as the original
    // because Factory::Manual just clones the Arc
    assert!(Arc::ptr_eq(&trc_relocated.into_arc(), &value));
}

#[test]
fn test_relocated_unknown_source() {
    let threads = test_threads(&[2]);

    let source = None;
    let destination = threads[1].clone();

    let mut trc = PerThreadArc::with_value(42);

    trc.relocate(source, &destination);
    assert_eq!(*trc, 42);
}

#[test]
fn test_strong_count() {
    // Test strong_count with a single reference
    let arc = PerThreadArc::new(Counter::new);
    assert_eq!(PerThreadArc::strong_count(&arc), 1);

    // Test strong_count with multiple references
    let arc2 = arc.clone();
    assert_eq!(PerThreadArc::strong_count(&arc), 2);
    assert_eq!(PerThreadArc::strong_count(&arc2), 2);

    let arc3 = arc.clone();
    assert_eq!(PerThreadArc::strong_count(&arc), 3);
    assert_eq!(PerThreadArc::strong_count(&arc2), 3);
    assert_eq!(PerThreadArc::strong_count(&arc3), 3);

    // Test strong_count after dropping a reference
    drop(arc2);
    assert_eq!(PerThreadArc::strong_count(&arc), 2);
    assert_eq!(PerThreadArc::strong_count(&arc3), 2);

    drop(arc3);
    assert_eq!(PerThreadArc::strong_count(&arc), 1);
}

#[test]
fn test_strong_count_after_relocation() {
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create an Arc with multiple strong references
    let arc1 = PerThreadArc::new(Counter::new);
    let arc2 = arc1.clone();
    assert_eq!(PerThreadArc::strong_count(&arc1), 2);

    // Relocate one of them
    let mut arc1_relocated = arc1;
    arc1_relocated.relocate(thread1.as_ref(), &thread2);

    // After relocation:
    // - arc1_relocated holds a reference to a new Arc created for thread2
    // - The storage at thread2 also holds a reference, but strong_count excludes internal refs
    // - Therefore, strong_count for arc1_relocated is 1
    assert_eq!(PerThreadArc::strong_count(&arc1_relocated), 1);

    // arc2 refers to the original Arc at thread1
    // - arc2 itself holds a reference
    // - The storage at thread1 also holds a reference, but strong_count excludes internal refs
    // - Therefore, strong_count for arc2 is 1
    assert_eq!(PerThreadArc::strong_count(&arc2), 1);
}

#[test]
fn test_strong_count_with_deduplication() {
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create an Arc and clone it
    let arc1 = PerThreadArc::new(Counter::new);
    let arc2 = arc1.clone();

    // Relocate both to the same destination
    // They should share the same underlying Arc in the destination
    let mut arc1_relocated = arc1;
    arc1_relocated.relocate(thread1.as_ref(), &thread2);
    let mut arc2_relocated = arc2;
    arc2_relocated.relocate(thread1.as_ref(), &thread2);

    // Both should point to the same underlying Arc (deduplication)
    // The strong count includes:
    // - arc1_relocated (1)
    // - arc2_relocated (1)
    // The storage reference for thread2 is excluded by strong_count.
    assert_eq!(PerThreadArc::strong_count(&arc1_relocated), 2);
    assert_eq!(PerThreadArc::strong_count(&arc2_relocated), 2);
}

#[test]
fn test_strong_count_independent_across_threads() {
    let threads = test_threads(&[2]);
    let thread1 = Some(threads[0].clone());
    let thread2 = threads[1].clone();

    // Create an Arc on thread1 with strong_count = 1.
    let arc_a = PerThreadArc::new(Counter::new);
    assert_eq!(PerThreadArc::strong_count(&arc_a), 1);

    // Relocate to thread2, creating a separate instance there.
    let mut arc_b = arc_a.clone();
    arc_b.relocate(thread1.as_ref(), &thread2);
    assert_eq!(PerThreadArc::strong_count(&arc_b), 1); // arc_b only; storage ref excluded

    // Clone arc_a on thread1 - this should not affect arc_b on thread2.
    let arc_a2 = arc_a.clone();
    // arc_a is now referenced by:
    // - arc_a itself
    // - arc_a2
    // The storage entry for thread1 also holds a reference, but strong_count excludes internal refs.
    assert_eq!(PerThreadArc::strong_count(&arc_a), 2);
    assert_eq!(PerThreadArc::strong_count(&arc_a2), 2);
    // arc_b on thread2 is unaffected by the clone on thread1.
    assert_eq!(PerThreadArc::strong_count(&arc_b), 1); // Still 1; unaffected by the clone on thread1.
}

#[test]
fn self_relocation_keeps_the_value_and_storage_consistent() {
    // Source and destination with the same key (here the same thread) are not a cross-key move,
    // so relocation keeps the carried value and seeds the key with it. The Arc and its key then
    // agree, and a later relocation finds that same value on the shared-probe fast path rather than
    // a stale or freshly materialized one.
    let threads = test_threads(&[2]);
    let thread = threads[0].clone();

    let arc = PerThreadArc::new(Counter::new);
    arc.increment_by(42);
    assert_eq!(arc.value(), 42);

    let mut arc = arc;
    arc.relocate(Some(&thread), &thread);
    assert_eq!(arc.value(), 42, "a same-key relocation keeps the carried value");

    arc.relocate(Some(&thread), &thread);
    assert_eq!(
        arc.value(),
        42,
        "the key holds the carried value, so a later relocation finds it unchanged"
    );
}

#[test]
fn with_clone_fn_relocates_clone() {
    let threads = test_threads(&[2]);
    let source = Some(threads[0].clone());
    let destination = threads[1].clone();

    // Counter::relocated resets value to 0, so we can detect if it was called.
    let arc = super::Arc::<Counter, crate::PerThread>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));

    arc.increment_by(42);
    assert_eq!(arc.value(), 42);

    // Relocating should clone the Counter and call relocated() on the clone,
    // which resets the value to 0.
    let mut arc = arc;
    arc.relocate(source.as_ref(), &destination);
    assert_eq!(arc.value(), 0, "relocated() must be called on the clone");
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
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
            self.0 = format!("{}-relocated", self.0);
        }
    }

    let threads = test_threads(&[2]);
    let source = Some(threads[0].clone());
    let destination = threads[1].clone();

    let arc = super::Arc::<dyn Plugin, crate::PerThread>::with_clone_fn(MyPlugin("orig".into()), |p: &MyPlugin| Box::new(p.clone()));

    assert_eq!(arc.name(), "orig");

    let mut arc = arc;
    arc.relocate(source.as_ref(), &destination);
    assert_eq!(arc.name(), "orig-relocated");
}

#[test]
fn with_clone_fn_clone_and_relocate_independently() {
    // Cloning an Arc backed by ErasedCloneFn should produce independent
    // clones that can each be relocated separately.
    let threads = test_threads(&[3]);
    let source = Some(threads[0].clone());
    let dest1 = threads[1].clone();
    let dest2 = threads[2].clone();

    let arc = super::Arc::<Counter, crate::PerThread>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    arc.increment_by(10);

    let mut clone1 = arc.clone();
    #[expect(clippy::redundant_clone, reason = "testing independent clones")]
    let mut clone2 = arc.clone();

    clone1.relocate(source.as_ref(), &dest1);
    clone2.relocate(source.as_ref(), &dest2);

    // Both should have been reset by Counter::relocate
    assert_eq!(clone1.value(), 0);
    assert_eq!(clone2.value(), 0);
}

#[test]
fn with_clone_fn_repeated_relocations() {
    // Multiple sequential relocations through the same ErasedCloneFn factory
    // must all produce correct clones.
    let threads = test_threads(&[4]);

    let arc = super::Arc::<Counter, crate::PerThread>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    arc.increment_by(99);

    let mut current = arc;
    for i in 0..3 {
        let source = Some(threads[i].clone());
        let dest = threads[i + 1].clone();
        current.relocate(source.as_ref(), &dest);
        // Counter resets to 0 on relocate
        assert_eq!(current.value(), 0, "relocation {i} should reset counter");
        current.increment_by(i32::try_from(i + 1).expect("loop index fits in i32"));
    }
    assert_eq!(current.value(), 3);
}

#[test]
fn with_clone_fn_debug_format() {
    // Exercises Debug formatting of the ErasedCloneFn factory path.
    let arc = super::Arc::<Counter, crate::PerThread>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    let debug = format!("{arc:?}");
    assert!(!debug.is_empty());
}

#[test]
fn with_clone_fn_deduplication_across_clones() {
    // Two clones relocated to the same destination should share the same
    // underlying value via storage deduplication.
    let threads = test_threads(&[2]);
    let source = Some(threads[0].clone());
    let dest = threads[1].clone();

    let arc = super::Arc::<Counter, crate::PerThread>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    let clone1 = arc.clone();
    #[expect(clippy::redundant_clone, reason = "testing independent clones")]
    let clone2 = arc.clone();

    let mut r1 = clone1;
    r1.relocate(source.as_ref(), &dest);
    let mut r2 = clone2;
    r2.relocate(source.as_ref(), &dest);

    assert!(sync::Arc::ptr_eq(&r1.clone().into_arc(), &r2.clone().into_arc()));
}

#[test]
fn with_clone_fn_debug_includes_erased() {
    // Exercises Debug for ErasedCloneFn (clone_fn.rs) and Factory::ErasedCloneFn (factory.rs)
    let arc = super::Arc::<Counter, crate::PerThread>::with_clone_fn(Counter::new(), |c: &Counter| Box::new(c.clone()));
    let dbg = format!("{arc:?}");
    assert!(
        dbg.contains("factory: Clone"),
        "Debug output should mention factory Clone variant: {dbg}"
    );
}

#[test]
fn factory_data_debug() {
    // Exercises Factory::Data debug branch
    let arc = PerThreadArc::from_unaware(42);
    let dbg = format!("{arc:?}");
    assert!(dbg.contains("Data"), "Debug output should mention Data variant: {dbg}");
}

#[test]
fn factory_manual_debug() {
    // Exercises Factory::Manual debug branch (from_storage)
    use std::sync::{self};

    let threads = test_threads(&[1]);
    let storage = sync::Arc::new(super::storage::Storage::new());
    storage.insert(&threads[0], sync::Arc::new(42)).unwrap();
    let arc = super::Arc::<i32, crate::PerThread>::from_storage(storage, &threads[0]);
    let dbg = format!("{arc:?}");
    assert!(dbg.contains("Manual"), "Debug output should mention Manual variant: {dbg}");
}

#[test]
fn factory_closure_debug() {
    // Exercises Factory::Closure debug branch (from Arc::new)
    let arc = PerThreadArc::new(Counter::new);
    let dbg = format!("{arc:?}");
    assert!(dbg.contains("Closure"), "Debug output should mention Closure variant: {dbg}");
}

#[test]
#[cfg_attr(
    all(miri, target_os = "windows"),
    ignore = "contended DashMap locking calls GetModuleHandleA, which Windows Miri does not support"
)]
fn concurrent_relocation_to_same_thread_materializes_once() {
    // Races many threads into the same empty keyed destination partition and asserts two things:
    // the caller's factory runs exactly once for that strategy partition, and every racer ends on
    // the one value published for it. The DashMap vacant-entry guard serializes first publication:
    // the winner runs the factory while holding the shard for writing, and every other racer then
    // observes and adopts the winner's `sync::Arc`. This is the documented "once per strategy
    // partition" contract holding under contention.
    // Ref: docs/implementation.md, "Publication and reentrancy".

    // A cloneable factory input that counts how many times the factory runs. Its `relocate` is a
    // no-op, so relocations do not disturb the count.
    #[derive(Clone)]
    struct Materializations(sync::Arc<AtomicUsize>);

    impl ThreadAware for Materializations {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    // Enough racers to make a publish/adopt race likely on a machine that can run several threads,
    // while staying cheap enough for a unit test.
    const RACERS: usize = 8;

    // A few rounds sample scheduler variation without turning the unit test into a stress test.
    const ROUNDS: usize = 4;

    let threads = test_threads(&[2]);
    let source = threads[0].clone();
    let destination = threads[1].clone();

    for _ in 0..ROUNDS {
        let materializations = sync::Arc::new(AtomicUsize::new(0));

        let origin = PerThreadArc::new_with(
            Materializations(sync::Arc::clone(&materializations)),
            |counter: Materializations| {
                counter.0.fetch_add(1, Ordering::AcqRel);
                Counter::new()
            },
        );
        origin.increment_by(7);

        // Construction materialized the origin's own value once; only destination materializations
        // should be counted, so start the race from zero.
        materializations.store(0, Ordering::Release);

        let barrier = sync::Arc::new(sync::Barrier::new(RACERS));

        let mut racers = Vec::with_capacity(RACERS);

        for _ in 0..RACERS {
            let mut racer = origin.clone();
            let barrier = sync::Arc::clone(&barrier);
            let source = source.clone();
            let destination = destination.clone();
            racers.push(std::thread::spawn(move || {
                // Release all racers together so they contend for the same empty destination
                // partition.
                barrier.wait();
                racer.relocate(Some(&source), &destination);
                racer.into_arc()
            }));
        }

        let values = racers.into_iter().map(|racer| racer.join().unwrap()).collect::<Vec<_>>();
        let (first, rest) = values.split_first().unwrap();

        for other in rest {
            assert!(
                sync::Arc::ptr_eq(first, other),
                "every racer must adopt the single value published for the destination strategy partition"
            );
        }

        assert_eq!(
            materializations.load(Ordering::Acquire),
            1,
            "the factory must run exactly once for the destination strategy partition, even under a race"
        );
        assert_eq!(first.value(), 0, "the destination value is freshly relocated, not the source value");
        assert_eq!(origin.value(), 7, "the source value is left intact");
    }
}

#[test]
fn later_relocations_reproduce_the_original_source_thread() {
    // The closure factory records the thread it first relocated from, so every later relocation
    // reproduces that original transfer instead of taking the `Arc`'s current thread as the
    // source. This pins the sequential propagation of that recorded source; the concurrent adopting
    // path is covered separately below.
    // Ref: docs/implementation.md, "Relocation".

    let threads = test_threads(&[3]);
    let a = threads[0].clone();
    let b = threads[1].clone();
    let c = threads[2].clone();

    let recorded = sync::Arc::new(sync::Mutex::new(None));
    let mut arc = PerThreadArc::new_with(SourceRecorder(sync::Arc::clone(&recorded)), |_recorder: SourceRecorder| 0_i32);

    // First relocation A -> B: nothing is recorded yet, so the given source A is used and stored.
    arc.relocate(Some(&a), &b);
    assert_eq!(
        *recorded.lock().unwrap(),
        Some(a.clone()),
        "the first relocation materializes with the source it was given"
    );

    // Relocate onward B -> C. The factory must reproduce the original source A, not the current
    // thread B.
    arc.relocate(Some(&b), &c);
    assert_eq!(
        *recorded.lock().unwrap(),
        Some(a.clone()),
        "a later relocation reproduces the original source thread, not the current one"
    );
}

#[test]
fn hit_path_records_the_original_source_thread() {
    // Pre-materialize B with one clone, then let another clone first relocate A -> B through the hit
    // path. Its later B -> C miss must still relocate constructor state from the original source A.
    let threads = test_threads(&[3]);
    let a = threads[0].clone();
    let b = threads[1].clone();
    let c = threads[2].clone();

    let recorded = sync::Arc::new(sync::Mutex::new(None));
    let origin = PerThreadArc::new_with(SourceRecorder(sync::Arc::clone(&recorded)), |_recorder: SourceRecorder| 0_i32);

    let mut publisher = origin.clone();
    publisher.relocate(Some(&a), &b);

    *recorded.lock().unwrap() = None;
    let mut arc = origin;
    arc.relocate(Some(&a), &b);
    assert_eq!(*recorded.lock().unwrap(), None, "a populated destination must not run the factory");

    arc.relocate(Some(&b), &c);
    assert_eq!(
        *recorded.lock().unwrap(),
        Some(a.clone()),
        "a clone whose first relocation was a hit must retain its original source"
    );
}

#[test]
fn same_partition_path_records_the_original_source_thread() {
    // A and B share one NUMA partition, while C belongs to another. The same-partition A -> B fast
    // path must record A so the later cross-partition B -> C materialization still relocates
    // constructor state from the original source.
    let threads = test_threads(&[2, 1]);
    let a = threads[0].clone();
    let b = threads[1].clone();
    let c = threads[2].clone();

    let recorded = sync::Arc::new(sync::Mutex::new(None));
    let mut arc =
        crate::Arc::<_, crate::PerNumaNode>::new_with(SourceRecorder(sync::Arc::clone(&recorded)), |_recorder: SourceRecorder| 0_i32);

    arc.relocate(Some(&a), &b);
    assert_eq!(
        *recorded.lock().unwrap(),
        None,
        "a same-partition relocation must keep the carried value without running the factory"
    );

    arc.relocate(Some(&b), &c);
    assert_eq!(
        *recorded.lock().unwrap(),
        Some(a.clone()),
        "a same-partition first relocation must retain the original source"
    );
}

#[test]
fn adopting_racer_retains_its_own_original_source_thread() {
    // Pause one clone after it records A as its factory source but before it probes the empty
    // destination partition. A second clone publishes B from C, forcing the paused clone to adopt
    // that value when it resumes. Its later B -> D miss must still relocate its own factory state
    // from A rather than inheriting the publisher's C provenance.
    let threads = test_threads(&[4]);
    let adopter_source = threads[0].clone();
    let publisher_source = threads[1].clone();
    let shared_destination = threads[2].clone();
    let later_destination = threads[3].clone();

    let recorded = sync::Arc::new(sync::Mutex::new(None));
    let origin = PerThreadArc::new_with(SourceRecorder(sync::Arc::clone(&recorded)), |_recorder: SourceRecorder| 0_i32);
    let mut adopter = origin.clone();
    let mut publisher = origin;

    let (paused_sender, paused_receiver) = sync::mpsc::sync_channel(0);
    let (resume_sender, resume_receiver) = sync::mpsc::sync_channel(0);
    let adopter_source_for_thread = adopter_source.clone();
    let shared_destination_for_thread = shared_destination.clone();
    let adopter_thread = std::thread::spawn(move || {
        let hook_guard = super::arc::set_after_factory_update_hook(move || {
            paused_sender.send(()).unwrap();
            resume_receiver.recv().unwrap();
        });

        adopter.relocate(Some(&adopter_source_for_thread), &shared_destination_for_thread);
        drop(hook_guard);
        adopter
    });

    paused_receiver.recv().unwrap();
    publisher.relocate(Some(&publisher_source), &shared_destination);
    let published = sync::Arc::clone(&publisher.value);

    *recorded.lock().unwrap() = None;
    resume_sender.send(()).unwrap();
    let mut adopter = adopter_thread.join().unwrap();
    assert!(
        sync::Arc::ptr_eq(&adopter.value, &published),
        "the paused racer must adopt the value published for the shared destination"
    );

    adopter.relocate(Some(&shared_destination), &later_destination);
    assert_eq!(
        *recorded.lock().unwrap(),
        Some(adopter_source),
        "an adopting racer must retain its own original source for later materialization"
    );
}

#[test]
fn dropping_factory_update_hook_guard_clears_the_hook() {
    let calls = sync::Arc::new(AtomicUsize::new(0));

    {
        let _hook_guard = super::arc::set_after_factory_update_hook({
            let calls = sync::Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        });
    }

    super::arc::run_after_factory_update_hook();
    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "dropping the guard must clear a hook that relocation did not consume"
    );
}

#[test]
fn stale_factory_update_hook_guard_preserves_a_new_registration() {
    let calls = sync::Arc::new(AtomicUsize::new(0));

    let first_guard = super::arc::set_after_factory_update_hook({
        let calls = sync::Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::AcqRel);
        }
    });
    super::arc::run_after_factory_update_hook();

    let second_guard = super::arc::set_after_factory_update_hook({
        let calls = sync::Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::AcqRel);
        }
    });

    // Consuming the first hook releases its thread-local slot before its guard is dropped. The
    // stale guard must recognize that the newly registered hook has a different owner.
    drop(first_guard);
    super::arc::run_after_factory_update_hook();
    drop(second_guard);

    assert_eq!(
        calls.load(Ordering::Acquire),
        2,
        "a stale guard must not clear a newer hook registration"
    );
}

#[test]
fn new_boxed_relocate() {
    // Exercises Ctor<T>::relocate (the no-op ThreadAware impl inside new_boxed)
    let threads = test_threads(&[2]);
    let mut arc = super::Arc::<Counter, crate::PerThread>::new_boxed(|| Box::new(Counter::new()));
    arc.relocate(Some(&threads[0]), &threads[1]);
    assert_eq!(arc.value(), 0, "new_boxed relocate should create a fresh counter");
}

#[test]
fn factory_panic_leaves_the_destination_partition_empty() {
    // A relocation that misses runs the factory while holding the vacant destination entry. The
    // factory is caller code and may panic; unwinding drops the entry guard without publishing a
    // value, so a later relocation can retry.

    // A value whose clone panics. `with_value` clones it only when materializing a new thread,
    // so construction succeeds and the first relocation into an empty thread panics.
    struct Bomb;

    impl Clone for Bomb {
        fn clone(&self) -> Self {
            panic!("materialization bomb");
        }
    }

    impl ThreadAware for Bomb {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    let threads = test_threads(&[2]);
    let source = threads[0].clone();
    let destination = threads[1].clone();

    let arc = super::Arc::<Bomb, crate::PerThread>::with_value(Bomb);
    let mut relocated = arc.clone();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relocated.relocate(Some(&source), &destination)));
    assert!(result.is_err(), "the factory panic must propagate to the caller");

    // The factory panicked before it could publish anything, so the destination stays empty and a
    // later relocation is free to materialize into it.
    assert!(
        arc.storage.get(&destination).is_none(),
        "a panicking factory must leave the destination partition empty"
    );
}

#[test]
fn relocation_preserves_the_source_thread_value() {
    // A miss records the value the Arc moved away from under the source thread's key, so a later
    // relocation back into that thread finds the original value instead of re-materializing a
    // fresh one. This guards the `source != destination` branch that performs that write.
    let threads = test_threads(&[2]);
    let source = threads[0].clone();
    let destination = threads[1].clone();

    let mut arc = PerThreadArc::new(Counter::new);

    // The value the Arc currently holds belongs to the source thread.
    let source_value = sync::Arc::clone(&arc.value);

    // Relocate away from the source thread: this materializes the destination and must record
    // `source_value` under the source key.
    arc.relocate(Some(&source), &destination);
    assert!(
        !sync::Arc::ptr_eq(&arc.value, &source_value),
        "relocating away must adopt the destination value"
    );

    // Relocate a clone back into the source thread. It must find the recorded original value, not
    // a freshly materialized one.
    let mut back = arc.clone();
    back.relocate(Some(&destination), &source);
    assert!(
        sync::Arc::ptr_eq(&back.value, &source_value),
        "relocating back into the source thread must find the preserved original value"
    );
}

#[test]
fn relocation_leaves_a_populated_source_key_untouched() {
    // Recording the moved-from value under the source key uses a write-once `set`, so an
    // already-populated source key is left as-is. Another thread may have recorded the same key
    // first; keeping the existing value is correct and must not overwrite it.
    let threads = test_threads(&[2]);
    let source = threads[0].clone();
    let destination = threads[1].clone();

    let mut arc = PerThreadArc::new(Counter::new);
    arc.increment_by(11);

    // Pre-populate the source key with a distinct value, standing in for a value another thread
    // already recorded there.
    let seeded = sync::Arc::new(Counter::new());
    seeded.increment_by(555);
    let _ = arc.storage.insert(&source, sync::Arc::clone(&seeded));

    // Relocate away from the source thread. The miss records the carried value under the source
    // key, but the key is already populated, so the pre-existing value must survive.
    arc.relocate(Some(&source), &destination);
    assert!(
        sync::Arc::ptr_eq(&arc.storage.get(&source).unwrap(), &seeded),
        "a populated source key must keep its existing value"
    );

    // A relocation back into the source thread must find the pre-existing value, confirming the
    // carried value never displaced it.
    let mut back = arc.clone();
    back.relocate(Some(&destination), &source);
    assert_eq!(back.value(), 555, "relocating back must find the pre-existing source value");
}

#[test]
#[cfg_attr(
    all(miri, target_os = "windows"),
    ignore = "contended DashMap locking calls GetModuleHandleA, which Windows Miri does not support"
)]
fn opposite_direction_relocations_converge_without_deadlock() {
    // Two threads relocate in opposite directions across the same pair of thread coordinates. These
    // factories do not reenter storage while a destination entry is held, and source recording
    // retains no map guard across another access, so each thread completes and ends on its
    // destination's published value.
    // Ref: docs/implementation.md, "Publication and reentrancy".

    // A small bounded repetition samples both writes without turning this into a stress test.
    const ROUNDS: usize = 8;

    let threads = test_threads(&[2]);
    let x = threads[0].clone();
    let y = threads[1].clone();

    for _ in 0..ROUNDS {
        let shared = PerThreadArc::new(Counter::new);
        let mut to_y = shared.clone();
        let mut to_x = shared.clone();

        let barrier = sync::Arc::new(sync::Barrier::new(2));
        let barrier_xy = sync::Arc::clone(&barrier);
        let barrier_yx = sync::Arc::clone(&barrier);

        let source = x.clone();
        let destination = y.clone();
        let xy = std::thread::spawn(move || {
            barrier_xy.wait();
            to_y.relocate(Some(&source), &destination);
            to_y.into_arc()
        });
        let source = y.clone();
        let destination = x.clone();
        let yx = std::thread::spawn(move || {
            barrier_yx.wait();
            to_x.relocate(Some(&source), &destination);
            to_x.into_arc()
        });

        // The joins return only once both threads finish; a deadlock would hang here instead.
        let landed_on_y = xy.join().unwrap();
        let landed_on_x = yx.join().unwrap();

        assert!(
            sync::Arc::ptr_eq(&landed_on_y, &shared.storage.get(&y).unwrap()),
            "the X->Y thread must end on the value published for Y"
        );
        assert!(
            sync::Arc::ptr_eq(&landed_on_x, &shared.storage.get(&x).unwrap()),
            "the Y->X thread must end on the value published for X"
        );
    }
}

#[test]
fn same_key_relocation_keeps_the_carried_value() {
    // Source and destination that resolve to the same key are not a cross-key move: the carried
    // value already belongs to that key, so relocation must keep it rather than materialize a
    // fresh one. `PerProcess` maps every thread to key 0, so any relocation exercises this.
    let threads = test_threads(&[2]);

    let arc = crate::Arc::<Counter, crate::PerProcess>::new(Counter::new);
    arc.increment_by(5);

    let mut moved = arc.clone();
    moved.relocate(Some(&threads[0]), &threads[1]);

    assert!(
        sync::Arc::ptr_eq(&moved.value, &arc.value),
        "a same-key relocation must keep the carried value, not materialize a fresh one"
    );
    assert_eq!(moved.value(), 5, "the shared value must be preserved across a same-key relocation");
}

#[test]
fn none_source_single_key_relocation_keeps_the_carried_value() {
    // A relocation with no source under a guaranteed single-partition strategy is still a
    // same-partition case: the carried value provably belongs to that partition, so `PerProcess`
    // must keep it rather than run the factory. This pins the `None`-source arm of the
    // `SINGLE_PARTITION` path, the whole of relocation under `PerProcess` for sourceless moves.
    // Ref: docs/design.md, `PerProcess`.
    let threads = test_threads(&[1]);

    let arc = crate::Arc::<Counter, crate::PerProcess>::new(Counter::new);
    arc.increment_by(5);

    let mut moved = arc.clone();
    moved.relocate(None, &threads[0]);

    assert!(
        sync::Arc::ptr_eq(&moved.value, &arc.value),
        "a sourceless single-key relocation must keep the carried value, not materialize a fresh one"
    );
    assert_eq!(
        moved.value(),
        5,
        "the shared value must be preserved across a sourceless single-key relocation"
    );
}

#[test]
fn none_source_multi_key_relocation_materializes_a_fresh_value() {
    // Under a partitioned strategy, a sourceless relocation cannot prove that the carried value
    // belongs to the destination key, so it must materialize a fresh value. This pins the
    // `SINGLE_PARTITION` marker boundary against accidentally treating keyed storage as though its
    // current contents described the complete coordinate space.
    let threads = test_threads(&[2]);

    let arc = PerThreadArc::new(Counter::new);
    arc.increment_by(5);

    let mut moved = arc.clone();
    moved.relocate(None, &threads[1]);

    assert!(
        !sync::Arc::ptr_eq(&moved.value, &arc.value),
        "a sourceless multi-key relocation must materialize a fresh value, not keep the carried one"
    );
    assert_eq!(moved.value(), 0, "a fresh PerThreadArc value starts independent of the source");
}

#[test]
fn cross_key_relocation_materializes_a_fresh_value() {
    // The counterpart to the same-key cases: a relocation between distinct keys must still
    // materialize an independent value for the destination. This guards against the same-key
    // short-circuit widening to cover genuine cross-key moves.
    let threads = test_threads(&[2]);

    let arc = PerThreadArc::new(Counter::new);
    arc.increment_by(5);

    let mut moved = arc.clone();
    moved.relocate(Some(&threads[0]), &threads[1]);

    assert!(
        !sync::Arc::ptr_eq(&moved.value, &arc.value),
        "a cross-key relocation must materialize a fresh value, not keep the carried one"
    );
    assert_eq!(moved.value(), 0, "a fresh PerThreadArc value starts independent of the source");
}
