// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;
#[cfg(feature = "std")]
use std::collections::HashMap;
#[cfg(feature = "std")]
use std::path::{Path, PathBuf};

use crate::{Affinity, ThreadAware};

// To make impl_transfer(...) work
macro_rules! impl_transfer {
    ($t:ty) => {
        impl ThreadAware for $t {
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
        }
    };
}

impl_transfer!(bool);
impl_transfer!(u8);
impl_transfer!(u16);
impl_transfer!(u32);
impl_transfer!(u64);
impl_transfer!(i8);
impl_transfer!(i16);
impl_transfer!(i32);
impl_transfer!(i64);
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
impl_transfer!(&Path);

impl_transfer!(&'static str);

// We need to implement `ThreadAware` for tuples ranging from 0 to 12 elements
macro_rules! impl_transfer_tuple {
    ($head:ident, $($tail:ident,)*) => {
        impl<$head, $($tail),*> ThreadAware for ($head, $($tail),*)
            where
                $head: ThreadAware,
                $($tail: ThreadAware),*
                {
                    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
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
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
        }
    };
}

impl_transfer_tuple!(A, B, C, D, E, F, G, H, I, J, K, L,);

macro_rules! impl_transfer_fn {
    ($head:ident, $($tail:ident,)*) => {
        impl<R, $head, $($tail),*> ThreadAware for fn($head, $($tail),*) -> R {
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
        }

        // Recursively call the macro for the rest of the function parameters
        impl_transfer_fn!($($tail,)*);
    };
    () => {
        impl<R> ThreadAware for fn() -> R {
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
        }
    }
}

impl_transfer_fn!(A, B, C, D, E, F, G, H, I, J, K, L,);

//TODO impl_transfer_array! macro to implement ThreadAware for arrays

impl<T> ThreadAware for Option<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
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
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
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
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Box<T>
where
    T: ThreadAware + ?Sized,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        (**self).relocate(source, destination);
    }
}

#[cfg(feature = "std")]
#[expect(
    clippy::implicit_hasher,
    reason = "This implementation preserves the existing API for the default hasher."
)]
impl<K, V> ThreadAware for HashMap<K, V>
where
    K: ThreadAware + Eq + core::hash::Hash,
    V: ThreadAware,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        let old = core::mem::take(self);
        for (mut key, mut value) in old {
            key.relocate(source, destination);
            value.relocate(source, destination);
            self.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::{Affinity, ThreadAware};

    fn pinned_affinities() -> [Affinity; 2] {
        [Affinity::new(0, 0, 2, 1), Affinity::new(1, 0, 2, 1)]
    }

    #[test]
    #[cfg(feature = "std")]
    fn test_hashmap() {
        use std::collections::HashMap;

        let affinities = pinned_affinities();
        let source = Some(affinities[0]);
        let destination = affinities[1];

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
        use crate::ThreadAware;
        let affinities = pinned_affinities();
        let source = Some(affinities[0]);
        let destination = affinities[1];

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
        use crate::ThreadAware;

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

        let affinities = pinned_affinities();
        let source = Some(affinities[0]);
        let destination = affinities[1];

        // Test fn() -> R (line 90)
        let mut fn_ptr_no_args: fn() -> i32 = no_args;
        fn_ptr_no_args.relocate(source, destination);
        assert_eq!(fn_ptr_no_args(), 42);

        // Test fn(A) -> R (line 80)
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
        use crate::ThreadAware;

        let affinities = pinned_affinities();
        let source = Some(affinities[0]);
        let destination = affinities[1];

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
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct Tracker(bool);

    impl ThreadAware for Tracker {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.0 = true;
        }
    }

    fn affinities() -> (Option<Affinity>, Affinity) {
        let a = pinned_affinities();
        (Some(a[0]), a[1])
    }

    #[test]
    fn option_some_forwards_relocate() {
        let (src, dst) = affinities();
        let mut val = Some(Tracker(false));
        val.relocate(src, dst);
        assert_eq!(val, Some(Tracker(true)));
    }

    #[test]
    fn option_none_is_noop() {
        let (src, dst) = affinities();
        let mut val: Option<Tracker> = None;
        val.relocate(src, dst);
        assert_eq!(val, None);
    }

    #[test]
    fn result_ok_forwards_relocate() {
        let (src, dst) = affinities();
        let mut val: Result<Tracker, Tracker> = Ok(Tracker(false));
        val.relocate(src, dst);
        assert_eq!(val, Ok(Tracker(true)));
    }

    #[test]
    fn result_err_forwards_relocate() {
        let (src, dst) = affinities();
        let mut val: Result<Tracker, Tracker> = Err(Tracker(false));
        val.relocate(src, dst);
        assert_eq!(val, Err(Tracker(true)));
    }

    #[test]
    fn vec_forwards_relocate_to_elements() {
        let (src, dst) = affinities();
        let mut val = vec![Tracker(false), Tracker(false)];
        val.relocate(src, dst);
        assert!(val.iter().all(|t| t.0), "all elements must be relocated");
    }

    #[test]
    fn box_forwards_relocate() {
        let (src, dst) = affinities();
        let mut val: Box<Tracker> = Box::new(Tracker(false));
        val.relocate(src, dst);
        assert!(val.0, "Box must forward relocate to inner value");
    }

    #[test]
    #[cfg(feature = "std")]
    fn hashmap_forwards_relocate_to_keys_and_values() {
        use std::collections::HashMap;

        let (src, dst) = affinities();
        let mut map = HashMap::new();
        map.insert(Tracker(false), Tracker(false));
        map.relocate(src, dst);
        for (key, value) in &map {
            assert!(key.0, "key must be relocated");
            assert!(value.0, "value must be relocated");
        }
    }
}
