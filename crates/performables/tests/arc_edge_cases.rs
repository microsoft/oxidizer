// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ownership transitions and unwind cleanup for process-wide Arc.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc as StdArc, Barrier};
use std::thread::spawn;

use performables::arc::{Arc, Weak};

#[derive(Debug)]
struct Payload(StdArc<AtomicUsize>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn make_mut_disassociates_weak_references_without_cloning_the_value() {
    #[derive(Debug)]
    struct Value {
        value: usize,
        clones: StdArc<AtomicUsize>,
    }
    impl Clone for Value {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::Relaxed);
            Self {
                value: self.value,
                clones: StdArc::clone(&self.clones),
            }
        }
    }
    let clones = StdArc::new(AtomicUsize::new(0));
    let mut value = Arc::new(Value {
        value: 7,
        clones: StdArc::clone(&clones),
    });
    let weak = Arc::downgrade(&value);
    assert!(Arc::get_mut(&mut value).is_none());
    Arc::make_mut(&mut value).value = 11;
    assert!(weak.upgrade().is_none());
    assert_eq!((value.value, clones.load(Ordering::Relaxed)), (11, 0));
}

#[test]
fn a_panicking_copy_on_write_clone_preserves_existing_owners_and_weak_references() {
    #[derive(Debug)]
    struct Value(usize);
    impl Clone for Value {
        fn clone(&self) -> Self {
            panic!("test a copy-on-write clone failing");
        }
    }
    let mut first = Arc::new(Value(13));
    let second = first.clone();
    let weak = Arc::downgrade(&first);
    catch_unwind(AssertUnwindSafe(|| {
        Arc::make_mut(&mut first);
    }))
    .unwrap_err();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!((first.0, second.0, Arc::strong_count(&first)), (13, 13, 2));
    assert!(Arc::ptr_eq(&weak.upgrade().unwrap(), &first));
}

#[test]
fn a_cyclic_constructor_cannot_upgrade_before_the_value_exists() {
    let saved = RefCell::new(None);
    let value = Arc::new_cyclic(|weak| {
        assert!(weak.upgrade().is_none());
        assert_eq!(weak.strong_count(), 0);
        *saved.borrow_mut() = Some(weak.clone());
        17
    });
    let weak = saved.into_inner().unwrap();
    assert!(Arc::ptr_eq(&value, &weak.upgrade().unwrap()));
    drop(value);
    assert!(weak.upgrade().is_none());
}

#[test]
fn weak_references_from_a_panicking_cyclic_constructor_remain_dead() {
    let saved = RefCell::<Option<Weak<usize>>>::new(None);
    catch_unwind(AssertUnwindSafe(|| {
        let _value = Arc::new_cyclic(|weak| {
            *saved.borrow_mut() = Some(weak.clone());
            panic!("test a cyclic constructor failing");
        });
    }))
    .unwrap_err();
    let weak = saved.into_inner().unwrap();
    assert_eq!(weak.strong_count(), 0);
    assert!(weak.upgrade().is_none());
}

#[test]
fn panicking_slice_collection_destroys_all_previously_constructed_values() {
    let drops = StdArc::new(AtomicUsize::new(0));
    catch_unwind(AssertUnwindSafe(|| {
        let _slice = (0..8)
            .map(|index| {
                assert_ne!(index, 5, "test iteration failing after five constructed values");
                Payload(StdArc::clone(&drops))
            })
            .collect::<Arc<[_]>>();
    }))
    .unwrap_err();
    assert_eq!(drops.load(Ordering::Relaxed), 5);
}

#[test]
fn concurrent_into_inner_calls_return_the_payload_to_exactly_one_owner() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let first = Arc::new(Payload(StdArc::clone(&drops)));
    let second = first.clone();
    let start = StdArc::new(Barrier::new(2));
    let worker = spawn({
        let start = StdArc::clone(&start);
        move || {
            start.wait();
            Arc::into_inner(second)
        }
    });
    start.wait();
    let first = Arc::into_inner(first);
    let second = worker.join().unwrap();
    assert_eq!(usize::from(first.is_some()) + usize::from(second.is_some()), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop((first, second));
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn shared_nan_values_do_not_become_equal_through_pointer_identity() {
    let first = Arc::new(f64::NAN);
    let second = first.clone();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(!first.eq(&second));
    assert_eq!(first.partial_cmp(&second), None);
}

testing_aids::init_tracing!();
