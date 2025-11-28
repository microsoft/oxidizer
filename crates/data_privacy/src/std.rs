// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{Classified, DataClass, RedactedDebug};
use std::fmt::Debug;

/// Data class for public/standard library types.
pub const PUBLIC: DataClass = DataClass::new("public", "data");

/// Implements Classified and RedactedDebug for non-generic std types.
macro_rules! impl_std_traits_debug_only {
    ($ty:ty, $data_class:expr) => {
        impl $crate::Classified for $ty {
            fn data_class(&self) -> $crate::DataClass {
                $data_class
            }
        }

        impl $crate::RedactedDebug for $ty {
            fn fmt(&self, _: &$crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                <Self as ::std::fmt::Debug>::fmt(self, f)
            }
        }
    };
}

/// Implements Classified, RedactedDebug, and RedactedDisplay for non-generic std types.
macro_rules! impl_std_traits {
    ($ty:ty, $data_class:expr) => {
        impl_std_traits_debug_only!($ty, $data_class);

        impl $crate::RedactedDisplay for $ty {
            fn fmt(&self, _: &$crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                <Self as ::std::fmt::Display>::fmt(self, f)
            }
        }
    };
}

/// Implements Classified and RedactedDebug for generic std types (no Display).
macro_rules! impl_std_traits_generic_debug_only {
    ($ty:ty, $data_class:expr, $($bounds:tt)*) => {
        impl<$($bounds)*> $crate::Classified for $ty {
            fn data_class(&self) -> $crate::DataClass {
                $data_class
            }
        }

        impl<$($bounds)*> $crate::RedactedDebug for $ty
        where
            $($bounds)*
        {
            fn fmt(&self, _engine: &$crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                <Self as ::std::fmt::Debug>::fmt(self, f)
            }
        }
    };
}


// Non-generic types with Display
impl_std_traits!(String, PUBLIC);
impl_std_traits!(&str, PUBLIC);
impl_std_traits!(bool, PUBLIC);
impl_std_traits!(char, PUBLIC);

// Unit type (no Display)
impl_std_traits_debug_only!((), PUBLIC);

// Integer types
impl_std_traits!(i8, PUBLIC);
impl_std_traits!(i16, PUBLIC);
impl_std_traits!(i32, PUBLIC);
impl_std_traits!(i64, PUBLIC);
impl_std_traits!(i128, PUBLIC);
impl_std_traits!(isize, PUBLIC);

impl_std_traits!(u8, PUBLIC);
impl_std_traits!(u16, PUBLIC);
impl_std_traits!(u32, PUBLIC);
impl_std_traits!(u64, PUBLIC);
impl_std_traits!(u128, PUBLIC);
impl_std_traits!(usize, PUBLIC);

// Float types
impl_std_traits!(f32, PUBLIC);
impl_std_traits!(f64, PUBLIC);

// Generic types (no Display)
impl_std_traits_generic_debug_only!(Vec<T>, PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(Option<T>, PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(&[T], PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(&mut [T], PUBLIC, T: RedactedDebug + Classified + Debug);

// Box can display if T can display (manual impl for ?Sized)
impl<T: ?Sized> crate::Classified for ::std::boxed::Box<T>
where
    T: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<T: ?Sized> crate::RedactedDebug for ::std::boxed::Box<T>
where
    T: crate::RedactedDebug + ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}

impl<T: ?Sized> crate::RedactedDisplay for ::std::boxed::Box<T>
where
    T: crate::RedactedDisplay + ::std::fmt::Display,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Display>::fmt(self, f)
    }
}

// Result with two type parameters (no Display)
impl<T, E> crate::Classified for ::std::result::Result<T, E>
where
    T: crate::Classified,
    E: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<T, E> crate::RedactedDebug for ::std::result::Result<T, E>
where
    T: crate::RedactedDebug + ::std::fmt::Debug,
    E: crate::RedactedDebug + ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}

// Smart pointers (can display if T can display, manual impl for ?Sized)
impl<T: ?Sized> crate::Classified for ::std::rc::Rc<T>
where
    T: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<T: ?Sized> crate::RedactedDebug for ::std::rc::Rc<T>
where
    T: crate::RedactedDebug + ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}

impl<T: ?Sized> crate::RedactedDisplay for ::std::rc::Rc<T>
where
    T: crate::RedactedDisplay + ::std::fmt::Display,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Display>::fmt(self, f)
    }
}

impl<T: ?Sized> crate::Classified for ::std::sync::Arc<T>
where
    T: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<T: ?Sized> crate::RedactedDebug for ::std::sync::Arc<T>
where
    T: crate::RedactedDebug + ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}

impl<T: ?Sized> crate::RedactedDisplay for ::std::sync::Arc<T>
where
    T: crate::RedactedDisplay + ::std::fmt::Display,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Display>::fmt(self, f)
    }
}

// Cow with lifetime (no Display in general)
impl<'a, T> crate::Classified for ::std::borrow::Cow<'a, T>
where
    T: ::std::borrow::ToOwned + ?Sized,
    T::Owned: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<'a, T> crate::RedactedDebug for ::std::borrow::Cow<'a, T>
where
    T: ::std::borrow::ToOwned + ?Sized,
    ::std::borrow::Cow<'a, T>: ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}

// Collections (no Display)
impl_std_traits_generic_debug_only!(std::collections::VecDeque<T>, PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(std::collections::LinkedList<T>, PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(std::collections::HashSet<T>, PUBLIC, T: RedactedDebug + Classified + Debug + std::hash::Hash + Eq);
impl_std_traits_generic_debug_only!(std::collections::BTreeSet<T>, PUBLIC, T: RedactedDebug + Classified + Debug + Ord);

// Maps with two type parameters (no Display)
impl<K, V> crate::Classified for ::std::collections::HashMap<K, V>
where
    K: crate::Classified,
    V: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<K, V> crate::RedactedDebug for ::std::collections::HashMap<K, V>
where
    K: crate::RedactedDebug + ::std::fmt::Debug,
    V: crate::RedactedDebug + ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}

impl<K, V> crate::Classified for ::std::collections::BTreeMap<K, V>
where
    K: crate::Classified,
    V: crate::Classified,
{
    fn data_class(&self) -> crate::DataClass {
        PUBLIC
    }
}

impl<K, V> crate::RedactedDebug for ::std::collections::BTreeMap<K, V>
where
    K: crate::RedactedDebug + ::std::fmt::Debug,
    V: crate::RedactedDebug + ::std::fmt::Debug,
{
    fn fmt(&self, _engine: &crate::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        <Self as ::std::fmt::Debug>::fmt(self, f)
    }
}
