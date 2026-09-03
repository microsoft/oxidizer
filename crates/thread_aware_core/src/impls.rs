// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::num::{
    NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize, NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128,
    NonZeroUsize,
};
use core::time::Duration;
#[cfg(any(test, feature = "std"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "std"))]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::thread;
#[cfg(any(test, feature = "std"))]
use std::thread::ThreadId;

use crate::{NumaNode, Owner, Thread, ThreadAware};

/// Implements [`ThreadAware`] for a type that holds nothing bound to a thread, so a
/// move leaves it valid as-is and `relocate` has nothing to do.
macro_rules! impl_thread_aware {
    ($t:ty) => {
        impl ThreadAware for $t {
            #[inline]
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
    };
}

impl_thread_aware!(bool);
impl_thread_aware!(u8);
impl_thread_aware!(u16);
impl_thread_aware!(u32);
impl_thread_aware!(u64);
impl_thread_aware!(u128);
impl_thread_aware!(i8);
impl_thread_aware!(i16);
impl_thread_aware!(i32);
impl_thread_aware!(i64);
impl_thread_aware!(i128);
impl_thread_aware!(usize);
impl_thread_aware!(isize);
impl_thread_aware!(f32);
impl_thread_aware!(f64);
impl_thread_aware!(char);

impl_thread_aware!(NonZeroU8);
impl_thread_aware!(NonZeroU16);
impl_thread_aware!(NonZeroU32);
impl_thread_aware!(NonZeroU64);
impl_thread_aware!(NonZeroU128);
impl_thread_aware!(NonZeroUsize);
impl_thread_aware!(NonZeroI8);
impl_thread_aware!(NonZeroI16);
impl_thread_aware!(NonZeroI32);
impl_thread_aware!(NonZeroI64);
impl_thread_aware!(NonZeroI128);
impl_thread_aware!(NonZeroIsize);

impl_thread_aware!(String);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(PathBuf);
impl_thread_aware!(Duration);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(Path);

impl_thread_aware!(str);
// Immutable process-lifetime string labels have no referent state to relocate and cannot dangle.
// This narrow reference exception does not extend to borrowed or mutable references in general.
impl_thread_aware!(&'static str);

impl_thread_aware!(Owner);
impl_thread_aware!(NumaNode);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(ThreadId);
impl_thread_aware!(Thread);

impl<T: ?Sized> ThreadAware for PhantomData<T>
where
    Self: Send,
{
    #[inline]
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

macro_rules! impl_thread_aware_tuple {
    ($head:ident, $($tail:ident,)*) => {
        impl<$head, $($tail),*> ThreadAware for ($head, $($tail),*)
            where
                $head: ThreadAware,
                $($tail: ThreadAware),*
                {
                    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
                        #[expect(non_snake_case, reason = "Macro-generated code uses uppercase identifiers for tuple elements")]
                        let ($head, $($tail),*) = self;
                        $head.relocate(source, destination);
                        $( $tail.relocate(source, destination); )*
                    }
                }

                // Recursively call the macro for the rest of the tuple
                impl_thread_aware_tuple!($($tail,)*);
    };

    () => {
        impl ThreadAware for () {
            #[inline]
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
    };
}

macro_rules! impl_thread_aware_fn {
    ($head:ident, $($tail:ident,)*) => {
        impl<R, $head, $($tail),*> ThreadAware for fn($head, $($tail),*) -> R {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }

        // Recursively call the macro for the rest of the function parameters
        impl_thread_aware_fn!($($tail,)*);
    };
    () => {
        impl<R> ThreadAware for fn() -> R {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
    }
}

macro_rules! impl_thread_aware_arities {
    ($($parameter:ident),* $(,)?) => {
        impl_thread_aware_tuple!($($parameter,)*);
        impl_thread_aware_fn!($($parameter,)*);
    };
}

// Match the established `thread_aware` boundary so compatibility code can rely on the same
// tuple and safe function-pointer arities.
impl_thread_aware_arities!(A, B, C, D, E, F, G, H, I, J, K, L);

impl<T, const N: usize> ThreadAware for [T; N]
where
    T: ThreadAware,
{
    #[inline]
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.as_mut_slice().relocate(source, destination);
    }
}

impl<T> ThreadAware for [T]
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Option<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        if let Some(value) = self {
            value.relocate(source, destination);
        }
    }
}

impl<T, E> ThreadAware for Result<T, E>
where
    T: ThreadAware,
    E: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        match self {
            Ok(value) => value.relocate(source, destination),
            Err(err) => err.relocate(source, destination),
        }
    }
}

impl<T> ThreadAware for Vec<T>
where
    T: ThreadAware,
{
    #[inline]
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.as_mut_slice().relocate(source, destination);
    }
}

impl<T> ThreadAware for VecDeque<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Box<T>
where
    T: ThreadAware + ?Sized,
{
    #[inline]
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        (**self).relocate(source, destination);
    }
}

impl<K, V> ThreadAware for BTreeMap<K, V>
where
    K: Send,
    V: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}

/// Implements [`ThreadAware`] for a single-threaded interior-mutability wrapper.
///
/// `relocate` already holds `&mut self`, so the inner value is reachable through
/// `get_mut` without a borrow flag or a `Cell` round-trip.
macro_rules! impl_thread_aware_cell {
    ($t:ident) => {
        impl<T> ThreadAware for $t<T>
        where
            T: ThreadAware,
        {
            #[inline]
            fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
                self.get_mut().relocate(source, destination);
            }
        }
    };
}

impl_thread_aware_cell!(Cell);
impl_thread_aware_cell!(RefCell);

#[cfg(any(test, feature = "std"))]
impl<K, V, S> ThreadAware for HashMap<K, V, S>
where
    K: Send,
    V: ThreadAware,
    S: Send,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::collections::{BTreeMap, VecDeque};
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::{Cell, RefCell};
    use core::hash::BuildHasherDefault;
    use core::marker::PhantomData;
    use core::num::{
        NonZero, NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize, NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64,
        NonZeroU128, NonZeroUsize,
    };
    use std::collections::HashMap;
    use std::collections::hash_map::DefaultHasher;

    use super::thread;
    use crate::{NumaNode, Owner, Thread, ThreadAware};

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
        let owner = Owner::new();
        let thread = thread::current().id();
        [
            Thread::new(owner.clone(), thread, NumaNode::new(0)),
            Thread::new(owner, thread, NumaNode::new(1)),
        ]
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
        let mut marker = PhantomData::<MarkerOnly>;
        marker.relocate(Some(&threads[0]), &threads[1]);
    }

    #[test]
    fn hashmap_relocates_values_and_tolerates_empty() {
        let threads = sample_threads();
        let source = Some(&threads[0]);
        let destination = &threads[1];

        let mut value: HashMap<i32, String> = HashMap::new();
        value.insert(1, "one".to_string());
        value.insert(2, "two".to_string());

        value.relocate(source, destination);

        assert_eq!(value.get(&1), Some(&"one".to_string()));
        assert_eq!(value.get(&2), Some(&"two".to_string()));

        let mut empty_value: HashMap<i32, String> = HashMap::new();
        empty_value.relocate(source, destination);
        assert!(empty_value.is_empty());
    }

    #[test]
    fn tuples_of_every_supported_arity_relocate() {
        let threads = sample_threads();
        let source = Some(&threads[0]);
        let destination = &threads[1];

        // Test empty tuple
        let mut empty_tuple = ();
        empty_tuple.relocate(source, destination);

        // Test single element tuple
        let mut single = (42,);
        single.relocate(source, destination);
        assert_eq!(single, (42,));

        // Test two element tuple
        let mut two = (42, "hello".to_string());
        two.relocate(source, destination);
        assert_eq!(two, (42, "hello".to_string()));

        // Test three element tuple with different types
        let mut three = (1, "test".to_string(), 1.23);
        three.relocate(source, destination);
        assert_eq!(three, (1, "test".to_string(), 1.23));

        // Test larger tuple (6 elements)
        let mut six = (1, 2, 3, 4, 5, 6);
        six.relocate(source, destination);
        assert_eq!(six, (1, 2, 3, 4, 5, 6));

        // Test tuple with nested Vec (complex type)
        let mut nested = (vec![1, 2, 3], "data".to_string(), 100u64);
        nested.relocate(source, destination);
        assert_eq!(nested, (vec![1, 2, 3], "data".to_string(), 100u64));

        // Test tuple with Option
        let mut with_option = (Some(42), None::<String>, "value".to_string());
        with_option.relocate(source, destination);
        assert_eq!(with_option, (Some(42), None::<String>, "value".to_string()));

        // Test large tuple (12 elements - maximum supported)
        let mut twelve = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
        twelve.relocate(source, destination);
        assert_eq!(twelve, (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12));
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

        // Test Ok variant
        let mut ok_value: Result<String, i32> = Ok("success".to_string());
        ok_value.relocate(source, destination);
        assert_eq!(ok_value, Ok("success".to_string()));

        // Test Err variant
        let mut err_value: Result<String, i32> = Err(42);
        err_value.relocate(source, destination);
        assert_eq!(err_value, Err(42));

        // Test with complex types
        let mut ok_vec: Result<Vec<i32>, String> = Ok(vec![1, 2, 3]);
        ok_vec.relocate(source, destination);
        assert_eq!(ok_vec, Ok(vec![1, 2, 3]));

        let mut err_string: Result<Vec<i32>, String> = Err("error".to_string());
        err_string.relocate(source, destination);
        assert_eq!(err_string, Err("error".to_string()));
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
        let mut val: Option<Tracker> = None;
        val.relocate(Some(&threads[0]), &threads[1]);
        assert_eq!(val, None);
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
}
