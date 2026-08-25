// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::time::Duration;
#[cfg(feature = "std")]
use std::collections::HashMap;
#[cfg(feature = "std")]
use std::path::{Path, PathBuf};

use crate::{Core, Location, MemoryRegion, ThreadAware, Topology};

// To make impl_transfer(...) work
macro_rules! impl_transfer {
    ($t:ty) => {
        impl ThreadAware for $t {
            fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
        }
    };
}

impl_transfer!(bool);
impl_transfer!(u8);
impl_transfer!(u16);
impl_transfer!(u32);
impl_transfer!(u64);
impl_transfer!(u128);
impl_transfer!(i8);
impl_transfer!(i16);
impl_transfer!(i32);
impl_transfer!(i64);
impl_transfer!(i128);
impl_transfer!(usize);
impl_transfer!(isize);
impl_transfer!(f32);
impl_transfer!(f64);
impl_transfer!(char);

impl_transfer!(String);
#[cfg(feature = "std")]
impl_transfer!(PathBuf);
impl_transfer!(Duration);
#[cfg(feature = "std")]
impl_transfer!(Path);
#[cfg(feature = "std")]
impl_transfer!(&Path);

impl_transfer!(str);
impl_transfer!(&str);

impl_transfer!(Topology);
impl_transfer!(Core);
impl_transfer!(MemoryRegion);
impl_transfer!(Location);

// We need to implement `ThreadAware` for tuples ranging from 0 to 12 elements
macro_rules! impl_transfer_tuple {
    ($head:ident, $($tail:ident,)*) => {
        impl<$head, $($tail),*> ThreadAware for ($head, $($tail),*)
            where
                $head: ThreadAware,
                $($tail: ThreadAware),*
                {
                    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
                        #[expect(non_snake_case, reason = "Macro-generated code uses uppercase identifiers for tuple elements")]
                        let ($head, $($tail),*) = self;
                        $head.relocate(source, destination);
                        $( $tail.relocate(source, destination); )*
                    }
                }

                // Recursively call the macro for the rest of the tuple
                impl_transfer_tuple!($($tail,)*);
    };

    () => {
        impl ThreadAware for () {
            fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
        }
    };
}

impl_transfer_tuple!(A, B, C, D, E, F, G, H, I, J, K, L,);

macro_rules! impl_transfer_fn {
    ($head:ident, $($tail:ident,)*) => {
        impl<R, $head, $($tail),*> ThreadAware for fn($head, $($tail),*) -> R {
            fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
        }

        // Recursively call the macro for the rest of the function parameters
        impl_transfer_fn!($($tail,)*);
    };
    () => {
        impl<R> ThreadAware for fn() -> R {
            fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
        }
    }
}

impl_transfer_fn!(A, B, C, D, E, F, G, H, I, J, K, L,);

impl<T, const N: usize> ThreadAware for [T; N]
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        self.as_mut_slice().relocate(source, destination);
    }
}

impl<T> ThreadAware for [T]
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Option<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
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
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
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
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for VecDeque<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Box<T>
where
    T: ThreadAware + ?Sized,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        (**self).relocate(source, destination);
    }
}

impl<K, V> ThreadAware for BTreeMap<K, V>
where
    K: Send,
    V: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Cell<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        self.get_mut().relocate(source, destination);
    }
}

impl<T> ThreadAware for RefCell<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        self.get_mut().relocate(source, destination);
    }
}

impl<'a, B> ThreadAware for Cow<'a, B>
where
    B: ToOwned + Sync + ?Sized,
    B::Owned: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        if let Cow::Owned(value) = self {
            value.relocate(source, destination);
        }
    }
}

#[cfg(feature = "std")]
impl<K, V, S> ThreadAware for HashMap<K, V, S>
where
    K: Send,
    V: ThreadAware,
    S: Send,
{
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::Cow;
    use alloc::boxed::Box;
    use alloc::collections::{BTreeMap, VecDeque};
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::{Cell, RefCell};

    use crate::{Core, Location, MemoryRegion, ThreadAware, Topology};

    fn sample_locations() -> [Location; 2] {
        [
            Location::new(Topology::from(0), Core::from(0), MemoryRegion::from(0)),
            Location::new(Topology::from(0), Core::from(1), MemoryRegion::from(0)),
        ]
    }

    #[test]
    #[cfg(feature = "std")]
    fn test_hashmap() {
        use std::collections::HashMap;

        let locations = sample_locations();
        let source = Some(&locations[0]);
        let destination = &locations[1];

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
    fn test_tuples() {
        let locations = sample_locations();
        let source = Some(&locations[0]);
        let destination = &locations[1];

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

    #[test]
    fn test_function_pointers() {
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

        let locations = sample_locations();
        let source = Some(&locations[0]);
        let destination = &locations[1];

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
    fn test_result() {
        let locations = sample_locations();
        let source = Some(&locations[0]);
        let destination = &locations[1];

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

    /// A type whose `relocate` visibly mutates state, so mutation tests catch
    /// no-op replacements.
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct Tracker(bool);

    impl ThreadAware for Tracker {
        fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {
            self.0 = true;
        }
    }

    #[test]
    fn option_some_forwards_relocate() {
        let locations = sample_locations();
        let mut val = Some(Tracker(false));
        val.relocate(Some(&locations[0]), &locations[1]);
        assert_eq!(val, Some(Tracker(true)));
    }

    #[test]
    fn option_none_is_noop() {
        let locations = sample_locations();
        let mut val: Option<Tracker> = None;
        val.relocate(Some(&locations[0]), &locations[1]);
        assert_eq!(val, None);
    }

    #[test]
    fn result_ok_forwards_relocate() {
        let locations = sample_locations();
        let mut val: Result<Tracker, Tracker> = Ok(Tracker(false));
        val.relocate(Some(&locations[0]), &locations[1]);
        assert_eq!(val, Ok(Tracker(true)));
    }

    #[test]
    fn result_err_forwards_relocate() {
        let locations = sample_locations();
        let mut val: Result<Tracker, Tracker> = Err(Tracker(false));
        val.relocate(Some(&locations[0]), &locations[1]);
        assert_eq!(val, Err(Tracker(true)));
    }

    #[test]
    fn vec_forwards_relocate_to_elements() {
        let locations = sample_locations();
        let mut val = vec![Tracker(false), Tracker(false)];
        val.relocate(Some(&locations[0]), &locations[1]);
        assert!(val.iter().all(|t| t.0), "all elements must be relocated");
    }

    #[test]
    fn array_and_slice_forward_relocate_to_elements() {
        let locations = sample_locations();
        let mut val = [Tracker(false), Tracker(false)];
        val.relocate(Some(&locations[0]), &locations[1]);
        assert!(val.iter().all(|t| t.0), "all array elements must be relocated");

        let slice: &mut [Tracker] = &mut val;
        for value in slice.iter_mut() {
            value.0 = false;
        }
        slice.relocate(Some(&locations[0]), &locations[1]);
        assert!(slice.iter().all(|t| t.0), "all slice elements must be relocated");
    }

    #[test]
    fn vec_deque_forwards_relocate_to_elements() {
        let locations = sample_locations();
        let mut val = VecDeque::from([Tracker(false), Tracker(false)]);
        val.relocate(Some(&locations[0]), &locations[1]);
        assert!(val.iter().all(|t| t.0), "all elements must be relocated");
    }

    #[test]
    fn btree_map_relocates_values_without_mutating_keys() {
        let locations = sample_locations();
        let mut map = BTreeMap::new();
        map.insert(Tracker(false), Tracker(false));
        map.relocate(Some(&locations[0]), &locations[1]);

        let (key, value) = map.first_key_value().unwrap();
        assert!(!key.0, "key identity must remain stable");
        assert!(value.0, "value must be relocated");
    }

    #[test]
    fn cells_forward_relocate_to_inner_value() {
        let locations = sample_locations();

        let mut cell = Cell::new(Tracker(false));
        cell.relocate(Some(&locations[0]), &locations[1]);
        assert!(cell.into_inner().0);

        let mut ref_cell = RefCell::new(Tracker(false));
        ref_cell.relocate(Some(&locations[0]), &locations[1]);
        assert!(ref_cell.into_inner().0);
    }

    #[test]
    fn cow_relocates_only_owned_value() {
        let locations = sample_locations();
        let borrowed = Tracker(false);
        let mut borrowed_cow = Cow::Borrowed(&borrowed);
        borrowed_cow.relocate(Some(&locations[0]), &locations[1]);
        assert!(!borrowed_cow.as_ref().0);

        let mut owned_cow: Cow<'_, Tracker> = Cow::Owned(Tracker(false));
        owned_cow.relocate(Some(&locations[0]), &locations[1]);
        assert!(owned_cow.as_ref().0);
    }

    #[test]
    fn box_forwards_relocate() {
        let locations = sample_locations();
        let mut val: Box<Tracker> = Box::new(Tracker(false));
        val.relocate(Some(&locations[0]), &locations[1]);
        assert!(val.0, "Box must forward relocate to inner value");
    }

    #[test]
    #[cfg(feature = "std")]
    fn hashmap_relocates_values_without_mutating_keys() {
        use core::hash::BuildHasherDefault;
        use std::collections::HashMap;
        use std::hash::DefaultHasher;

        let locations = sample_locations();
        let mut map: HashMap<Tracker, Tracker, BuildHasherDefault<DefaultHasher>> = HashMap::default();
        map.insert(Tracker(false), Tracker(false));
        map.relocate(Some(&locations[0]), &locations[1]);

        let (key, value) = map.iter().next().unwrap();
        assert!(!key.0, "key identity must remain stable");
        assert!(value.0, "value must be relocated");
    }
}
