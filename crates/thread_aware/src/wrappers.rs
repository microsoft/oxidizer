// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::{MemoryAffinity, ThreadAware};

/// Allows transferring a value that doesn't implement [`ThreadAware`]
///
/// Since the [`ThreadAware`] trait is not commonly implemented, this wrapper can
/// be used to allow transferring values that don't implement [`ThreadAware`].
///
/// Care must be taken when using this type - since the value will be moved
/// as is, if it contains shared references to data other threads may use,
/// it can introduce contention, resulting in performance impact. As a rule
/// of thumb, if the wrapped value contains an Arc with interior mutability
/// somewhere inside, this wrapper should not be used, and a [`PerCore`](`crate::PerCore`) or [`PerNuma`](`crate::PerNuma`)
/// with independent initialization per affinity is a better option.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Unaware<T>(pub T);

// The only way this could cause issues is if you had a `fn generic<T>(x: impl From<Unaware<T>>)` or similar,
// which in practice is extremely unlikely to occur, especially since `Unaware` is not a common type.
impl<T> From<T> for Unaware<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for Unaware<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Unaware<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> ThreadAware for Unaware<T> {
    fn relocated(self, _source: MemoryAffinity, _destination: MemoryAffinity) -> Self {
        self
    }
}

impl<T> Unaware<T> {
    /// Consumes the wrapper and returns the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Converts an `Arc<Unaware<T>>` into an `Arc<T>`.
    pub fn into_arc(self: Arc<Self>) -> Arc<T> {
        // SAFETY: `Unaware` is a transparent wrapper around `T`,
        unsafe { std::mem::transmute(self) }
    }
}

/// Creates an [`Unaware`] wrapper around a value.
pub const fn unaware<T>(value: T) -> Unaware<T> {
    Unaware(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn test_unaware_construction() {
        // Test direct construction
        let value = Unaware(42);
        assert_eq!(value.0, 42);

        let string_value = Unaware("hello".to_string());
        assert_eq!(string_value.0, "hello");

        // Test via unaware() function
        let value = unaware(100);
        assert_eq!(value.0, 100);

        let vec_value = unaware(vec![1, 2, 3]);
        assert_eq!(vec_value.0, vec![1, 2, 3]);
    }

    #[test]
    fn test_unaware_deref() {
        let mut value = Unaware(vec![1, 2, 3]);

        // Test Deref
        assert_eq!(value.len(), 3);
        assert_eq!(value[0], 1);

        // Test DerefMut
        value.push(4);
        assert_eq!(value.len(), 4);
        assert_eq!(value[3], 4);
    }

    #[test]
    fn test_unaware_clone_and_copy() {
        // Test Clone
        let original = Unaware(42);
        let cloned = original;
        assert_eq!(original.0, 42);
        assert_eq!(cloned.0, 42);

        // Test Copy (both should be accessible)
        let original = Unaware(42);
        let copied = original;
        assert_eq!(original.0, 42);
        assert_eq!(copied.0, 42);
    }

    #[test]
    fn test_unaware_derived_traits() {
        use std::collections::HashSet;

        // Test Debug
        let value = Unaware(42);
        let debug_str = format!("{value:?}");
        assert!(debug_str.contains("Unaware"));
        assert!(debug_str.contains("42"));

        // Test PartialEq
        let value1 = Unaware(42);
        let value2 = Unaware(42);
        let value3 = Unaware(43);
        assert_eq!(value1, value2);
        assert_ne!(value1, value3);

        // Test Hash
        let mut set = HashSet::new();
        set.insert(Unaware(1));
        set.insert(Unaware(2));
        set.insert(Unaware(1)); // Duplicate
        assert_eq!(set.len(), 2);
        assert!(set.contains(&Unaware(1)));
        assert!(set.contains(&Unaware(2)));
    }

    #[test]
    fn test_unaware_default() {
        let default_i32: Unaware<i32> = Unaware::default();
        assert_eq!(default_i32.0, 0);

        let default_string: Unaware<String> = Unaware::default();
        assert_eq!(default_string.0, "");

        let default_vec: Unaware<Vec<i32>> = Unaware::default();
        assert_eq!(default_vec.0.len(), 0);
    }

    #[test]
    fn test_unaware_thread_aware() {
        let affinities = crate::create_manual_memory_affinities(&[2]);

        // Test with simple type
        let value = Unaware(42);
        let relocated = value.relocated(affinities[0], affinities[1]);
        assert_eq!(relocated.0, 42);

        // Test with String
        let value = Unaware("test string".to_string());
        let relocated = value.relocated(affinities[0], affinities[1]);
        assert_eq!(relocated.0, "test string");

        // Test with complex type (HashMap)
        let mut map = HashMap::new();
        map.insert("key1", 100);
        map.insert("key2", 200);
        let value = Unaware(map);
        let relocated = value.relocated(affinities[0], affinities[1]);
        assert_eq!(relocated.0.get("key1"), Some(&100));
        assert_eq!(relocated.0.get("key2"), Some(&200));
    }

    #[test]
    fn test_unaware_into_arc() {
        // Test with i32
        let value = Unaware(42);
        let arc_unaware = Arc::new(value);
        let arc_inner: Arc<i32> = arc_unaware.into_arc();
        assert_eq!(*arc_inner, 42);

        // Test with String
        let value = Unaware("hello".to_string());
        let arc_unaware = Arc::new(value);
        let arc_inner: Arc<String> = arc_unaware.into_arc();
        assert_eq!(*arc_inner, "hello");

        // Test with Vec
        let value = Unaware(vec![1, 2, 3, 4, 5]);
        let arc_unaware = Arc::new(value);
        let arc_inner: Arc<Vec<i32>> = arc_unaware.into_arc();

        assert_eq!(*arc_inner, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_unaware_into_inner_preserves_arc_count() {
        let value = Unaware(100);
        let arc_unaware = Arc::new(value);
        let _arc_clone = Arc::clone(&arc_unaware);

        assert_eq!(Arc::strong_count(&arc_unaware), 2);

        let arc_inner: Arc<i32> = arc_unaware.into_arc();

        // Arc count should be preserved (though one handle is now the inner type)
        assert_eq!(Arc::strong_count(&arc_inner), 2);
    }

    #[test]
    fn test_unaware_with_arc_inside() {
        // Test the warning case mentioned in docs - Arc with interior mutability
        let inner_arc = Arc::new(Mutex::new(42));
        let unaware_wrapper = Unaware(Arc::clone(&inner_arc));

        // Should work, but this is the case the docs warn about
        let affinities = crate::create_manual_memory_affinities(&[2]);
        let relocated = unaware_wrapper.relocated(affinities[0], affinities[1]);

        // Both should still point to the same underlying data
        // Original + clone in wrapper = 2, relocated is a copy (since Unaware<Arc<_>> implements Copy)
        assert_eq!(Arc::strong_count(&inner_arc), 2);
        assert_eq!(Arc::strong_count(&relocated.0), 2);
    }

    #[test]
    fn test_unaware_field_access() {
        let value = Unaware(vec![10, 20, 30]);

        // Direct field access
        assert_eq!(value.0[0], 10);
        assert_eq!(value.0[1], 20);
        assert_eq!(value.0[2], 30);

        // Via Deref
        assert_eq!(value.first(), Some(&10));
        assert_eq!(value.last(), Some(&30));
    }

    #[test]
    fn test_unaware_nested_structure() {
        #[derive(Debug, PartialEq)]
        struct Complex {
            id: i32,
            name: String,
            values: Vec<i32>,
        }

        let complex = Complex {
            id: 1,
            name: "test".to_string(),
            values: vec![1, 2, 3],
        };

        let unaware_complex = Unaware(complex);
        assert_eq!(unaware_complex.0.id, 1);
        assert_eq!(unaware_complex.0.name, "test");
        assert_eq!(unaware_complex.0.values, vec![1, 2, 3]);

        // Test with relocation
        let affinities = crate::create_manual_memory_affinities(&[2]);
        let relocated = unaware_complex.relocated(affinities[0], affinities[1]);
        assert_eq!(relocated.0.id, 1);
        assert_eq!(relocated.0.name, "test");
    }

    #[test]
    fn test_unaware_const_function() {
        // Test that unaware is a const function
        const VALUE: Unaware<i32> = unaware(42);
        assert_eq!(VALUE.0, 42);
    }

    #[test]
    fn test_unaware_into_inner() {
        let value = unaware(55);
        let inner = value.into_inner();
        assert_eq!(inner, 55);
    }

    #[test]
    fn test_from_impl() {
        let value: Unaware<i32> = 99.into();
        assert_eq!(value.0, 99);

        let string_value: Unaware<String> = "from impl".to_string().into();
        assert_eq!(string_value.0, "from impl");
    }
}
