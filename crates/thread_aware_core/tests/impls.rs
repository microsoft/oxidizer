// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises public [`ThreadAware`](thread_aware_core::ThreadAware) implementations from
//! outside the crate.

#![cfg(feature = "std")]

use core::cell::{Cell, RefCell};
use core::hash::BuildHasherDefault;
use core::marker::PhantomData;
use core::num::{
    NonZero, NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize, NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64,
    NonZeroU128, NonZeroUsize,
};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::thread;

use thread_aware_core::__private::v1::{new_numa_node, new_owner, new_thread};
use thread_aware_core::{Thread, ThreadAware};

/// A type whose `relocate` visibly mutates state, so mutation tests catch
/// no-op replacements.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Tracker(bool);

impl ThreadAware for Tracker {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        self.0 = true;
    }
}

fn sample_threads() -> [Thread; 2] {
    let owner = new_owner();
    let thread = thread::current().id();
    [
        new_thread(owner.clone(), thread, new_numa_node(0)),
        new_thread(owner, thread, new_numa_node(1)),
    ]
}

fn assert_relocation_is_noop<T>(mut value: T, source: Option<&Thread>, destination: &Thread)
where
    T: ThreadAware + Clone + PartialEq + core::fmt::Debug,
{
    let original = value.clone();

    value.relocate(source, destination);

    assert_eq!(value, original);
}

#[test]
fn all_nonzero_integer_types_are_thread_aware() {
    fn assert_thread_aware<T: ThreadAware>() {}

    assert_thread_aware::<NonZeroU8>();
    assert_thread_aware::<NonZeroU16>();
    assert_thread_aware::<NonZeroU32>();
    assert_thread_aware::<NonZeroU64>();
    assert_thread_aware::<NonZeroU128>();
    assert_thread_aware::<NonZeroUsize>();
    assert_thread_aware::<NonZeroI8>();
    assert_thread_aware::<NonZeroI16>();
    assert_thread_aware::<NonZeroI32>();
    assert_thread_aware::<NonZeroI64>();
    assert_thread_aware::<NonZeroI128>();
    assert_thread_aware::<NonZeroIsize>();

    assert_thread_aware::<NonZero<u32>>();
}

#[test]
fn phantom_data_is_thread_aware_without_requiring_its_type_to_be() {
    struct MarkerOnly;

    fn assert_thread_aware<T: ThreadAware>() {}

    assert_thread_aware::<PhantomData<MarkerOnly>>();

    let threads = sample_threads();
    assert_relocation_is_noop(PhantomData::<MarkerOnly>, Some(&threads[0]), &threads[1]);
}

#[test]
fn hashmap_relocates_values_and_tolerates_empty() {
    let threads = sample_threads();
    let source = Some(&threads[0]);
    let destination = &threads[1];

    let mut value = HashMap::new();
    value.insert(1, "one".to_string());
    value.insert(2, "two".to_string());

    assert_relocation_is_noop(value, source, destination);
    assert_relocation_is_noop(HashMap::<i32, String>::new(), source, destination);
}

#[test]
fn tuples_of_every_supported_arity_relocate() {
    let threads = sample_threads();
    let source = Some(&threads[0]);
    let destination = &threads[1];

    assert_relocation_is_noop((), source, destination);
    assert_relocation_is_noop((42,), source, destination);
    assert_relocation_is_noop((42, "hello".to_string()), source, destination);
    assert_relocation_is_noop((1, "test".to_string(), 1.23), source, destination);
    assert_relocation_is_noop((1, 2, 3, 4, 5, 6), source, destination);
    assert_relocation_is_noop((vec![1, 2, 3], "data".to_string(), 100u64), source, destination);
    assert_relocation_is_noop((Some(42), None::<String>, "value".to_string()), source, destination);
    assert_relocation_is_noop((1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12), source, destination);
}

/// `impl_thread_aware_tuple!` recurses by peeling one element off the head, so the
/// 1-tuple is the last impl it generates before the `()` base case. That arm is
/// the one where a missing comma would silently change meaning: `let (A) = self`
/// is a parenthesized pattern binding the whole tuple, which sends the impl back
/// into itself instead of reaching the element. `Tracker` makes the visit
/// observable, so this pins the head, the tail and the 12-element maximum.
#[test]
fn every_tuple_element_is_relocated() {
    let threads = sample_threads();
    let source = Some(&threads[0]);
    let destination = &threads[1];

    let mut one = (Tracker(false),);
    one.relocate(source, destination);
    assert_eq!(one, (Tracker(true),));

    let mut two = (Tracker(false), Tracker(false));
    two.relocate(source, destination);
    assert_eq!(two, (Tracker(true), Tracker(true)));

    let mut twelve = (
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
        Tracker(false),
    );
    twelve.relocate(source, destination);
    assert_eq!(
        twelve,
        (
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
            Tracker(true),
        )
    );
}

#[test]
fn function_pointers_stay_callable_after_relocate() {
    // Helper functions for testing
    fn no_args() -> i32 {
        42
    }

    fn one_arg(x: i32) -> i32 {
        x * 2
    }

    fn two_args(x: i32, y: i32) -> i32 {
        x + y
    }

    fn three_args(a: i32, b: i32, c: i32) -> i32 {
        a + b + c
    }

    fn many_args(arg0: i32, arg1: i32, arg2: i32, arg3: i32, arg4: i32, arg5: i32) -> i32 {
        arg0 + arg1 + arg2 + arg3 + arg4 + arg5
    }

    // Test with different return types
    fn returns_string() -> String {
        "hello".to_string()
    }

    fn returns_bool(x: i32) -> bool {
        x > 0
    }

    let threads = sample_threads();
    let source = Some(&threads[0]);
    let destination = &threads[1];

    // Test fn() -> R
    let mut fn_ptr_no_args: fn() -> i32 = no_args;
    fn_ptr_no_args.relocate(source, destination);
    assert_eq!(fn_ptr_no_args(), 42);

    // Test fn(A) -> R
    let mut fn_ptr_one: fn(i32) -> i32 = one_arg;
    fn_ptr_one.relocate(source, destination);
    assert_eq!(fn_ptr_one(5), 10);

    // Test fn(A, B) -> R
    let mut fn_ptr_two: fn(i32, i32) -> i32 = two_args;
    fn_ptr_two.relocate(source, destination);
    assert_eq!(fn_ptr_two(3, 7), 10);

    // Test fn(A, B, C) -> R
    let mut fn_ptr_three: fn(i32, i32, i32) -> i32 = three_args;
    fn_ptr_three.relocate(source, destination);
    assert_eq!(fn_ptr_three(1, 2, 3), 6);

    // Test with many arguments
    let mut fn_ptr_many: fn(i32, i32, i32, i32, i32, i32) -> i32 = many_args;
    fn_ptr_many.relocate(source, destination);
    assert_eq!(fn_ptr_many(1, 2, 3, 4, 5, 6), 21);

    let mut fn_string: fn() -> String = returns_string;
    fn_string.relocate(source, destination);
    assert_eq!(fn_string(), "hello".to_string());

    let mut fn_bool: fn(i32) -> bool = returns_bool;
    fn_bool.relocate(source, destination);
    assert!(fn_bool(5));
    assert!(!fn_bool(-3));
}

#[test]
fn result_relocates_both_variants() {
    let threads = sample_threads();
    let source = Some(&threads[0]);
    let destination = &threads[1];

    assert_relocation_is_noop(Result::<String, i32>::Ok("success".to_string()), source, destination);
    assert_relocation_is_noop(Result::<String, i32>::Err(42), source, destination);
    assert_relocation_is_noop(Result::<Vec<i32>, String>::Ok(vec![1, 2, 3]), source, destination);
    assert_relocation_is_noop(Result::<Vec<i32>, String>::Err("error".to_string()), source, destination);
}

#[test]
fn option_some_forwards_relocate() {
    let threads = sample_threads();
    let mut val = Some(Tracker(false));
    val.relocate(Some(&threads[0]), &threads[1]);
    assert_eq!(val, Some(Tracker(true)));
}

#[test]
fn option_none_is_noop() {
    let threads = sample_threads();
    assert_relocation_is_noop::<Option<Tracker>>(None, Some(&threads[0]), &threads[1]);
}

#[test]
fn result_ok_forwards_relocate() {
    let threads = sample_threads();
    let mut val: Result<Tracker, Tracker> = Ok(Tracker(false));
    val.relocate(Some(&threads[0]), &threads[1]);
    assert_eq!(val, Ok(Tracker(true)));
}

#[test]
fn result_err_forwards_relocate() {
    let threads = sample_threads();
    let mut val: Result<Tracker, Tracker> = Err(Tracker(false));
    val.relocate(Some(&threads[0]), &threads[1]);
    assert_eq!(val, Err(Tracker(true)));
}

#[test]
fn vec_forwards_relocate_to_elements() {
    let threads = sample_threads();
    let mut val = vec![Tracker(false), Tracker(false)];
    val.relocate(Some(&threads[0]), &threads[1]);
    assert!(val.iter().all(|t| t.0), "all elements must be relocated");
}

#[test]
fn array_and_slice_forward_relocate_to_elements() {
    let threads = sample_threads();
    let mut val = [Tracker(false), Tracker(false)];
    val.relocate(Some(&threads[0]), &threads[1]);
    assert!(val.iter().all(|t| t.0), "all array elements must be relocated");

    let val: &mut [Tracker] = &mut val;
    for value in val.iter_mut() {
        value.0 = false;
    }
    val.relocate(Some(&threads[0]), &threads[1]);
    assert!(val.iter().all(|t| t.0), "all slice elements must be relocated");
}

#[test]
fn vec_deque_forwards_relocate_to_elements() {
    let threads = sample_threads();
    let mut val = VecDeque::from([Tracker(false), Tracker(false)]);
    val.relocate(Some(&threads[0]), &threads[1]);
    assert!(val.iter().all(|t| t.0), "all elements must be relocated");
}

#[test]
fn btree_map_relocates_values_without_mutating_keys() {
    let threads = sample_threads();
    let mut map = BTreeMap::new();
    map.insert(Tracker(false), Tracker(false));
    map.relocate(Some(&threads[0]), &threads[1]);

    let (key, value) = map.first_key_value().unwrap();
    assert!(!key.0, "key identity must remain stable");
    assert!(value.0, "value must be relocated");
}

#[test]
fn cells_forward_relocate_to_inner_value() {
    let threads = sample_threads();

    let mut cell = Cell::new(Tracker(false));
    cell.relocate(Some(&threads[0]), &threads[1]);
    assert!(cell.into_inner().0);

    let mut ref_cell = RefCell::new(Tracker(false));
    ref_cell.relocate(Some(&threads[0]), &threads[1]);
    assert!(ref_cell.into_inner().0);
}

#[test]
fn box_forwards_relocate() {
    let threads = sample_threads();
    let mut val: Box<Tracker> = Box::new(Tracker(false));
    val.relocate(Some(&threads[0]), &threads[1]);
    assert!(val.0, "must forward relocate to the inner value");
}

#[test]
fn hashmap_relocates_values_without_mutating_keys() {
    let threads = sample_threads();
    let mut map: HashMap<Tracker, Tracker, BuildHasherDefault<DefaultHasher>> = HashMap::default();
    map.insert(Tracker(false), Tracker(false));
    map.relocate(Some(&threads[0]), &threads[1]);

    let (key, value) = map.iter().next().unwrap();
    assert!(!key.0, "key identity must remain stable");
    assert!(value.0, "value must be relocated");
}
