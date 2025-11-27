// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Debug, Display, Formatter};
use crate::{Classified, DataClass, RedactedDebug, RedactedDisplay, RedactionEngine};

/// Data class for public/standard library types.
pub const PUBLIC: DataClass = DataClass::new("public", "data");

/// Implements Classified and RedactedDebug for non-generic std types.
macro_rules! impl_std_traits_debug_only {
    ($ty:ty, $data_class:expr) => {
        impl Classified for $ty {
            fn data_class(&self) -> DataClass {
                $data_class
            }
        }

        impl RedactedDebug for $ty {
            fn fmt(&self, _: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
                <Self as Debug>::fmt(self, f)
            }
        }
    };
}

/// Implements Classified, RedactedDebug, and RedactedDisplay for non-generic std types.
macro_rules! impl_std_traits {
    ($ty:ty, $data_class:expr) => {
        impl_std_traits_debug_only!($ty, $data_class);

        impl RedactedDisplay for $ty {
            fn fmt(&self, _: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
                <Self as Display>::fmt(self, f)
            }
        }
    };
}

/// Implements Classified and RedactedDebug for generic std types (no Display).
macro_rules! impl_std_traits_generic_debug_only {
    ($ty:ty, $data_class:expr, $($bounds:tt)*) => {
        impl<$($bounds)*> Classified for $ty {
            fn data_class(&self) -> DataClass {
                $data_class
            }
        }

        impl<$($bounds)*> RedactedDebug for $ty
        where
            $($bounds)*
        {
            fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
                <Self as Debug>::fmt(self, f)
            }
        }
    };
}

/// Implements Classified, RedactedDebug, and RedactedDisplay for generic std types.
macro_rules! impl_std_traits_generic {
    ($ty:ty, $data_class:expr, $($bounds:tt)*) => {
        impl_std_traits_generic_debug_only!($ty, $data_class, $($bounds)*);

        impl<$($bounds)*> RedactedDisplay for $ty
        where
            $($bounds)*
        {
            fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
                <Self as Display>::fmt(self, f)
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
impl<T: ?Sized> Classified for Box<T>
where
    T: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<T: ?Sized> RedactedDebug for Box<T>
where
    T: RedactedDebug + Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

impl<T: ?Sized> RedactedDisplay for Box<T>
where
    T: RedactedDisplay + Display,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

// Result with two type parameters (no Display)
impl<T, E> Classified for Result<T, E>
where
    T: Classified,
    E: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<T, E> RedactedDebug for Result<T, E>
where
    T: RedactedDebug + Debug,
    E: RedactedDebug + Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

// Smart pointers (can display if T can display, manual impl for ?Sized)
impl<T: ?Sized> Classified for std::rc::Rc<T>
where
    T: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<T: ?Sized> RedactedDebug for std::rc::Rc<T>
where
    T: RedactedDebug + Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

impl<T: ?Sized> RedactedDisplay for std::rc::Rc<T>
where
    T: RedactedDisplay + Display,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

impl<T: ?Sized> Classified for std::sync::Arc<T>
where
    T: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<T: ?Sized> RedactedDebug for std::sync::Arc<T>
where
    T: RedactedDebug + Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

impl<T: ?Sized> RedactedDisplay for std::sync::Arc<T>
where
    T: RedactedDisplay + Display,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

// Cow with lifetime (no Display in general)
impl<'a, T> Classified for std::borrow::Cow<'a, T>
where
    T: ToOwned + ?Sized,
    T::Owned: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<'a, T> RedactedDebug for std::borrow::Cow<'a, T>
where
    T: ToOwned + ?Sized,
    std::borrow::Cow<'a, T>: Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

// Collections (no Display)
impl_std_traits_generic_debug_only!(std::collections::VecDeque<T>, PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(std::collections::LinkedList<T>, PUBLIC, T: RedactedDebug + Classified + Debug);
impl_std_traits_generic_debug_only!(std::collections::HashSet<T>, PUBLIC, T: RedactedDebug + Classified + Debug + std::hash::Hash + Eq);
impl_std_traits_generic_debug_only!(std::collections::BTreeSet<T>, PUBLIC, T: RedactedDebug + Classified + Debug + Ord);

// Maps with two type parameters (no Display)
impl<K, V> Classified for std::collections::HashMap<K, V>
where
    K: Classified,
    V: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<K, V> RedactedDebug for std::collections::HashMap<K, V>
where
    K: RedactedDebug + Debug,
    V: RedactedDebug + Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

impl<K, V> Classified for std::collections::BTreeMap<K, V>
where
    K: Classified,
    V: Classified,
{
    fn data_class(&self) -> DataClass {
        PUBLIC
    }
}

impl<K, V> RedactedDebug for std::collections::BTreeMap<K, V>
where
    K: RedactedDebug + Debug,
    V: RedactedDebug + Debug,
{
    fn fmt(&self, _engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

