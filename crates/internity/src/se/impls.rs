// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`SerializeIn`] implementations for [`Sym`], scalars, and common containers.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use serde::ser::{Error as _, SerializeTuple};
use serde::{Serialize, Serializer};

use super::{SerializeIn, SerializeInWith};
use crate::{Reader, Sym};

impl<R: Reader + ?Sized> SerializeIn<R> for Sym {
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match reader.try_resolve(*self) {
            Some(value) => serializer.serialize_str(value),
            None => Err(S::Error::custom(
                "internity: Sym is out of range for this reader (unresolvable handle)",
            )),
        }
    }
}

macro_rules! serialize_via_serde {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl<R: Reader + ?Sized> SerializeIn<R> for $ty {
                #[inline]
                fn serialize_in<S>(&self, _reader: &R, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: Serializer,
                {
                    <$ty as Serialize>::serialize(self, serializer)
                }
            }
        )+
    };
}

serialize_via_serde!(
    (),
    bool,
    char,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64,
    str,
    String,
);

impl<T, R> SerializeIn<R> for Option<T>
where
    T: SerializeIn<R>,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Some(value) => serializer.serialize_some(&SerializeInWith::new(value, reader)),
            None => serializer.serialize_none(),
        }
    }
}

impl<T, R> SerializeIn<R> for Box<T>
where
    T: SerializeIn<R>,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        T::serialize_in(self, reader, serializer)
    }
}

impl<T, R> SerializeIn<R> for Vec<T>
where
    T: SerializeIn<R>,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.iter().map(|value| SerializeInWith::new(value, reader)))
    }
}

impl<K, V, R> SerializeIn<R> for BTreeMap<K, V>
where
    K: SerializeIn<R>,
    V: SerializeIn<R>,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(
            self.iter()
                .map(|(key, value)| (SerializeInWith::new(key, reader), SerializeInWith::new(value, reader))),
        )
    }
}

impl<T, R> SerializeIn<R> for BTreeSet<T>
where
    T: SerializeIn<R>,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.iter().map(|value| SerializeInWith::new(value, reader)))
    }
}

#[cfg(feature = "std")]
impl<K, V, H, R> SerializeIn<R> for std::collections::HashMap<K, V, H>
where
    K: SerializeIn<R>,
    V: SerializeIn<R>,
    H: core::hash::BuildHasher,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(
            self.iter()
                .map(|(key, value)| (SerializeInWith::new(key, reader), SerializeInWith::new(value, reader))),
        )
    }
}

#[cfg(feature = "std")]
impl<T, H, R> SerializeIn<R> for std::collections::HashSet<T, H>
where
    T: SerializeIn<R>,
    H: core::hash::BuildHasher,
    R: Reader + ?Sized,
{
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.iter().map(|value| SerializeInWith::new(value, reader)))
    }
}

macro_rules! tuple_impl {
    ($count:expr, $($ty:ident $index:tt),+) => {
        impl<R, $($ty,)+> SerializeIn<R> for ($($ty,)+)
        where
            R: Reader + ?Sized,
            $($ty: SerializeIn<R>,)+
        {
            fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut tuple = serializer.serialize_tuple($count)?;
                $(
                    tuple.serialize_element(&SerializeInWith::new(&self.$index, reader))?;
                )+
                tuple.end()
            }
        }
    };
}

tuple_impl!(1, T0 0);
tuple_impl!(2, T0 0, T1 1);
tuple_impl!(3, T0 0, T1 1, T2 2);
tuple_impl!(4, T0 0, T1 1, T2 2, T3 3);
tuple_impl!(5, T0 0, T1 1, T2 2, T3 3, T4 4);
tuple_impl!(6, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5);
tuple_impl!(7, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6);
tuple_impl!(8, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7);
tuple_impl!(9, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8);
tuple_impl!(10, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9);
tuple_impl!(11, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9, T10 10);
tuple_impl!(12, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9, T10 10, T11 11);
tuple_impl!(13, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9, T10 10, T11 11, T12 12);
tuple_impl!(14, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9, T10 10, T11 11, T12 12, T13 13);
tuple_impl!(15, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9, T10 10, T11 11, T12 12, T13 13, T14 14);
tuple_impl!(16, T0 0, T1 1, T2 2, T3 3, T4 4, T5 5, T6 6, T7 7, T8 8, T9 9, T10 10, T11 11, T12 12, T13 13, T14 14, T15 15);
