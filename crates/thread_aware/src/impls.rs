// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::affinity::{MemoryAffinity, PinnedAffinity};
use crate::core::ThreadAware;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, path::PathBuf};

// To make impl_transfer(...) work
macro_rules! impl_transfer {
    ($t:ty) => {
        impl ThreadAware for $t {
            fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
                self
            }
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
impl_transfer!(PathBuf);
impl_transfer!(Duration);
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
                    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
                        #[expect(non_snake_case, reason = "Macro-generated code uses uppercase identifiers for tuple elements")]
                        let ($head, $($tail),*) = self;
                        (
                            $head.relocated(source, destination),
                            $( $tail.relocated(source, destination), )*
                        )
                    }
                }

                // Recursively call the macro for the rest of the tuple
                impl_transfer_tuple!($($tail,)*);
    };

    () => {
        impl ThreadAware for () {
            fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
                self
            }
        }
    };
}

impl_transfer_tuple!(A, B, C, D, E, F, G, H, I, J, K, L,);

macro_rules! impl_transfer_fn {
    ($head:ident, $($tail:ident,)*) => {
        impl<R, $head, $($tail),*> ThreadAware for fn($head, $($tail),*) -> R {
            fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
                self
            }
        }

        // Recursively call the macro for the rest of the function parameters
        impl_transfer_fn!($($tail,)*);
    };
    () => {
        impl<R> ThreadAware for fn() -> R {
            fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
                self
            }
        }
    }
}

impl_transfer_fn!(A, B, C, D, E, F, G, H, I, J, K, L,);

//TODO impl_transfer_array! macro to implement ThreadAware for arrays

impl<T> ThreadAware for Option<T>
where
    T: ThreadAware,
{
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        self.map(|value| value.relocated(source, destination))
    }
}

impl<T, E> ThreadAware for Result<T, E>
where
    T: ThreadAware,
    E: ThreadAware,
{
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        match self {
            Ok(value) => Ok(value.relocated(source, destination)),
            Err(err) => Err(err.relocated(source, destination)),
        }
    }
}

impl<T> ThreadAware for Vec<T>
where
    T: ThreadAware,
{
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        let mut result = Self::with_capacity(self.len());
        for value in self {
            result.push(value.relocated(source, destination));
        }
        result
    }
}

// TODO: We should probably support custom hashers as well.
#[expect(
    clippy::implicit_hasher,
    reason = "Supporting custom hashers would complicate the implementation significantly."
)]
impl<K, V> ThreadAware for HashMap<K, V>
where
    K: ThreadAware + Eq + std::hash::Hash,
    V: ThreadAware,
{
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        let mut result = Self::with_capacity(self.len());
        for (key, value) in self {
            result.insert(key.relocated(source, destination), value.relocated(source, destination));
        }
        result
    }
}

impl<T> ThreadAware for Arc<T> {
    fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::affinity::pinned_affinities;

    #[test]
    #[cfg(feature = "threads")]
    fn test_hashmap() {
        use crate::ThreadAware;
        use std::collections::HashMap;

        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        let mut value: HashMap<i32, String> = HashMap::new();
        value.insert(1, "one".to_string());
        value.insert(2, "two".to_string());

        let transferred = value.relocated(source, destination);

        assert_eq!(transferred.get(&1), Some(&"one".to_string()));
        assert_eq!(transferred.get(&2), Some(&"two".to_string()));

        let empty_value: HashMap<i32, String> = HashMap::new();
        let transferred_empty = empty_value.relocated(source, destination);
        assert_eq!(transferred_empty.len(), 0);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn test_tuples() {
        use crate::ThreadAware;
        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        // Test empty tuple
        let empty_tuple = ();
        let _: () = empty_tuple.relocated(source, destination);

        // Test single element tuple
        let single = (42,);
        let transferred_single = single.relocated(source, destination);
        assert_eq!(transferred_single, (42,));

        // Test two element tuple
        let two = (42, "hello".to_string());
        let transferred_two = two.relocated(source, destination);
        assert_eq!(transferred_two, (42, "hello".to_string()));

        // Test three element tuple with different types
        let three = (1, "test".to_string(), 1.23);
        let transferred_three = three.relocated(source, destination);
        assert_eq!(transferred_three, (1, "test".to_string(), 1.23));

        // Test larger tuple (6 elements)
        let six = (1, 2, 3, 4, 5, 6);
        let transferred_six = six.relocated(source, destination);
        assert_eq!(transferred_six, (1, 2, 3, 4, 5, 6));

        // Test tuple with nested Vec (complex type)
        let nested = (vec![1, 2, 3], "data".to_string(), 100u64);
        let transferred_nested = nested.relocated(source, destination);
        assert_eq!(transferred_nested, (vec![1, 2, 3], "data".to_string(), 100u64));

        // Test tuple with Option
        let with_option = (Some(42), None::<String>, "value".to_string());
        let transferred_option = with_option.relocated(source, destination);
        assert_eq!(transferred_option, (Some(42), None::<String>, "value".to_string()));

        // Test large tuple (12 elements - maximum supported)
        let twelve = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
        let transferred_twelve = twelve.relocated(source, destination);
        assert_eq!(transferred_twelve, (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12));
    }

    #[test]
    #[cfg(feature = "threads")]
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

        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        // Test fn() -> R (line 90)
        let fn_ptr_no_args: fn() -> i32 = no_args;
        let transferred_no_args = fn_ptr_no_args.relocated(source, destination);
        assert_eq!(transferred_no_args(), 42);
        // Verify it's the same function pointer
        assert_eq!(fn_ptr_no_args(), transferred_no_args());

        // Test fn(A) -> R (line 80)
        let fn_ptr_one: fn(i32) -> i32 = one_arg;
        let transferred_one = fn_ptr_one.relocated(source, destination);
        assert_eq!(transferred_one(5), 10);
        assert_eq!(fn_ptr_one(42), transferred_one(42));

        // Test fn(A, B) -> R
        let fn_ptr_two: fn(i32, i32) -> i32 = two_args;
        let transferred_two = fn_ptr_two.relocated(source, destination);
        assert_eq!(transferred_two(3, 7), 10);
        assert_eq!(fn_ptr_two(3, 4), transferred_two(3, 4));

        // Test fn(A, B, C) -> R
        let fn_ptr_three: fn(i32, i32, i32) -> i32 = three_args;
        let transferred_three = fn_ptr_three.relocated(source, destination);
        assert_eq!(transferred_three(1, 2, 3), 6);
        assert_eq!(fn_ptr_three(27, 5, 6), transferred_three(27, 5, 6));

        // Test with many arguments
        let fn_ptr_many: fn(i32, i32, i32, i32, i32, i32) -> i32 = many_args;
        let transferred_many = fn_ptr_many.relocated(source, destination);
        assert_eq!(transferred_many(1, 2, 3, 4, 5, 6), 21);
        assert_eq!(fn_ptr_many(1, 2, 3, 4, 5, 6), transferred_many(1, 2, 3, 4, 5, 6));

        let fn_string: fn() -> String = returns_string;
        let transferred_string = fn_string.relocated(source, destination);
        assert_eq!(transferred_string(), "hello".to_string());

        let fn_bool: fn(i32) -> bool = returns_bool;
        let transferred_bool = fn_bool.relocated(source, destination);
        assert!(transferred_bool(5));
        assert!(!transferred_bool(-3));
    }

    #[test]
    fn test_result() {
        use crate::ThreadAware;

        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        // Test Ok variant
        let ok_value: Result<String, i32> = Ok("success".to_string());
        let transferred_ok = ok_value.relocated(source, destination);
        assert_eq!(transferred_ok, Ok("success".to_string()));

        // Test Err variant
        let err_value: Result<String, i32> = Err(42);
        let transferred_err = err_value.relocated(source, destination);
        assert_eq!(transferred_err, Err(42));

        // Test with complex types
        let ok_vec: Result<Vec<i32>, String> = Ok(vec![1, 2, 3]);
        let transferred_ok_vec = ok_vec.relocated(source, destination);
        assert_eq!(transferred_ok_vec, Ok(vec![1, 2, 3]));

        let err_string: Result<Vec<i32>, String> = Err("error".to_string());
        let transferred_err_string = err_string.relocated(source, destination);
        assert_eq!(transferred_err_string, Err("error".to_string()));
    }

    #[test]
    fn test_arc() {
        use crate::ThreadAware;
        use std::sync::Arc;

        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        // Test Arc with simple type
        let arc_value = Arc::new(42);
        let arc_clone = Arc::clone(&arc_value);
        let transferred = arc_value.relocated(source, destination);

        // Arc should maintain reference count and point to the same data
        assert_eq!(*transferred, 42);
        assert_eq!(Arc::strong_count(&transferred), 2);
        assert_eq!(Arc::strong_count(&arc_clone), 2);

        // Test Arc with String
        let arc_string = Arc::new("hello".to_string());
        let transferred_string = arc_string.relocated(source, destination);
        assert_eq!(*transferred_string, "hello".to_string());

        // Test Arc with Vec
        let arc_vec = Arc::new(vec![1, 2, 3]);
        let transferred_vec = arc_vec.relocated(source, destination);
        assert_eq!(*transferred_vec, vec![1, 2, 3]);
    }
}
