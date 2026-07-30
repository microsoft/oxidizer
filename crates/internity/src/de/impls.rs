// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`DeserializeIn`] implementations for [`Sym`], scalars, and common
//! containers.

#![expect(
    clippy::renamed_function_params,
    clippy::use_self,
    reason = "Serde visitor parameter names describe collection roles, and explicit type names clarify recursive bounds"
)]

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

use serde::de::{Deserializer, Error as _, MapAccess, SeqAccess, Visitor};

use super::{DeserializeIn, DeserializeInSeed, cautious_capacity};
use crate::{Lexicon, Sym};

/// Interns the decoded string into the supplied interner.
struct SymVisitor<'a, I: ?Sized> {
    interner: &'a mut I,
}

impl<I: Lexicon + ?Sized> Visitor<'_> for SymVisitor<'_, I> {
    type Value = Sym;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string to intern")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(self.interner.intern(value))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(self.interner.intern(&value))
    }
}

impl<'de, I: Lexicon + ?Sized> DeserializeIn<'de, I> for Sym {
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SymVisitor { interner })
    }
}

macro_rules! deserialize_via_serde {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl<'de, I: Lexicon + ?Sized> DeserializeIn<'de, I> for $ty {
                #[inline]
                fn deserialize_in<D>(_interner: &mut I, deserializer: D) -> Result<Self, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    <$ty as serde::Deserialize>::deserialize(deserializer)
                }
            }
        )+
    };
}

deserialize_via_serde!(
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
    String,
);

impl<'de, T, I> DeserializeIn<'de, I> for Option<T>
where
    T: DeserializeIn<'de, I>,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionVisitor<'a, T, I: Lexicon + ?Sized> {
            interner: &'a mut I,
            marker: PhantomData<fn() -> T>,
        }

        impl<'de, T, I> Visitor<'de> for OptionVisitor<'_, T, I>
        where
            T: DeserializeIn<'de, I>,
            I: Lexicon + ?Sized,
        {
            type Value = Option<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an optional value")
            }

            #[cfg_attr(test, mutants::skip)] // `Option<T>::default()` is also `None`.
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            #[cfg_attr(test, mutants::skip)] // `Option<T>::default()` is also `None`.
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize_in(self.interner, deserializer).map(Some)
            }
        }

        deserializer.deserialize_option(OptionVisitor {
            interner,
            marker: PhantomData,
        })
    }
}

impl<'de, T, I> DeserializeIn<'de, I> for Box<T>
where
    T: DeserializeIn<'de, I>,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize_in(interner, deserializer).map(Box::new)
    }
}

impl<'de, T, I> DeserializeIn<'de, I> for Vec<T>
where
    T: DeserializeIn<'de, I>,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VecVisitor<'a, T, I: Lexicon + ?Sized> {
            interner: &'a mut I,
            marker: PhantomData<fn() -> T>,
        }

        impl<'de, T, I> Visitor<'de> for VecVisitor<'_, T, I>
        where
            T: DeserializeIn<'de, I>,
            I: Lexicon + ?Sized,
        {
            type Value = Vec<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::with_capacity(cautious_capacity::<T>(seq.size_hint()));
                while let Some(value) = seq.next_element_seed(DeserializeInSeed::<T, I>::new(&mut *self.interner))? {
                    values.push(value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_seq(VecVisitor {
            interner,
            marker: PhantomData,
        })
    }
}

impl<'de, K, V, I> DeserializeIn<'de, I> for BTreeMap<K, V>
where
    K: DeserializeIn<'de, I> + Ord,
    V: DeserializeIn<'de, I>,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MapVisitor<'a, K, V, I: Lexicon + ?Sized> {
            interner: &'a mut I,
            marker: PhantomData<fn() -> (K, V)>,
        }

        impl<'de, K, V, I> Visitor<'de> for MapVisitor<'_, K, V, I>
        where
            K: DeserializeIn<'de, I> + Ord,
            V: DeserializeIn<'de, I>,
            I: Lexicon + ?Sized,
        {
            type Value = BTreeMap<K, V>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some(key) = map.next_key_seed(DeserializeInSeed::<K, I>::new(&mut *self.interner))? {
                    let value = map.next_value_seed(DeserializeInSeed::<V, I>::new(&mut *self.interner))?;
                    values.insert(key, value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_map(MapVisitor {
            interner,
            marker: PhantomData,
        })
    }
}

impl<'de, T, I> DeserializeIn<'de, I> for BTreeSet<T>
where
    T: DeserializeIn<'de, I> + Ord,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SetVisitor<'a, T, I: Lexicon + ?Sized> {
            interner: &'a mut I,
            marker: PhantomData<fn() -> T>,
        }

        impl<'de, T, I> Visitor<'de> for SetVisitor<'_, T, I>
        where
            T: DeserializeIn<'de, I> + Ord,
            I: Lexicon + ?Sized,
        {
            type Value = BTreeSet<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = BTreeSet::new();
                while let Some(value) = seq.next_element_seed(DeserializeInSeed::<T, I>::new(&mut *self.interner))? {
                    values.insert(value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_seq(SetVisitor {
            interner,
            marker: PhantomData,
        })
    }
}

#[cfg(feature = "std")]
impl<'de, K, V, H, I> DeserializeIn<'de, I> for std::collections::HashMap<K, V, H>
where
    K: DeserializeIn<'de, I> + Eq + core::hash::Hash,
    V: DeserializeIn<'de, I>,
    H: core::hash::BuildHasher + Default,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MapVisitor<'a, K, V, H, I: Lexicon + ?Sized> {
            interner: &'a mut I,
            #[expect(clippy::type_complexity, reason = "the phantom carries all three collection type parameters")]
            marker: PhantomData<fn() -> (K, V, H)>,
        }

        impl<'de, K, V, H, I> Visitor<'de> for MapVisitor<'_, K, V, H, I>
        where
            K: DeserializeIn<'de, I> + Eq + core::hash::Hash,
            V: DeserializeIn<'de, I>,
            H: core::hash::BuildHasher + Default,
            I: Lexicon + ?Sized,
        {
            type Value = std::collections::HashMap<K, V, H>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                // No capacity prealloc: a hostile `size_hint` must not let the peer
                // force a large upfront allocation (mirrors the `BTreeMap` impl).
                let mut values = std::collections::HashMap::<K, V, H>::default();
                while let Some(key) = map.next_key_seed(DeserializeInSeed::<K, I>::new(&mut *self.interner))? {
                    let value = map.next_value_seed(DeserializeInSeed::<V, I>::new(&mut *self.interner))?;
                    let _ = values.insert(key, value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_map(MapVisitor {
            interner,
            marker: PhantomData,
        })
    }
}

#[cfg(feature = "std")]
impl<'de, T, H, I> DeserializeIn<'de, I> for std::collections::HashSet<T, H>
where
    T: DeserializeIn<'de, I> + Eq + core::hash::Hash,
    H: core::hash::BuildHasher + Default,
    I: Lexicon + ?Sized,
{
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SetVisitor<'a, T, H, I: Lexicon + ?Sized> {
            interner: &'a mut I,
            marker: PhantomData<fn() -> (T, H)>,
        }

        impl<'de, T, H, I> Visitor<'de> for SetVisitor<'_, T, H, I>
        where
            T: DeserializeIn<'de, I> + Eq + core::hash::Hash,
            H: core::hash::BuildHasher + Default,
            I: Lexicon + ?Sized,
        {
            type Value = std::collections::HashSet<T, H>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // No capacity prealloc, matching the `HashMap`/`BTreeSet` impls.
                let mut values = std::collections::HashSet::<T, H>::default();
                while let Some(value) = seq.next_element_seed(DeserializeInSeed::<T, I>::new(&mut *self.interner))? {
                    let _ = values.insert(value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_seq(SetVisitor {
            interner,
            marker: PhantomData,
        })
    }
}

macro_rules! tuple_impl {
    ($count:expr, $($ty:ident $index:tt),+) => {
        impl<'de, I, $($ty,)+> DeserializeIn<'de, I> for ($($ty,)+)
        where
            I: Lexicon + ?Sized,
            $($ty: DeserializeIn<'de, I>,)+
        {
            fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct TupleVisitor<'a, I: Lexicon + ?Sized, $($ty,)+> {
                    interner: &'a mut I,
                    marker: PhantomData<fn() -> ($($ty,)+)>,
                }

                impl<'de, I, $($ty,)+> Visitor<'de> for TupleVisitor<'_, I, $($ty,)+>
                where
                    I: Lexicon + ?Sized,
                    $($ty: DeserializeIn<'de, I>,)+
                {
                    type Value = ($($ty,)+);

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(formatter, "a tuple of size {}", $count)
                    }

                    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
                    where
                        A: SeqAccess<'de>,
                    {
                        Ok((
                            $(
                                seq.next_element_seed(DeserializeInSeed::<$ty, I>::new(&mut *self.interner))?
                                    .ok_or_else(|| A::Error::invalid_length($index, &self))?,
                            )+
                        ))
                    }
                }

                deserializer.deserialize_tuple(
                    $count,
                    TupleVisitor {
                        interner,
                        marker: PhantomData,
                    },
                )
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #[test]
    fn capacity_hint_is_capped() {
        assert_eq!(super::cautious_capacity::<u8>(Some(usize::MAX)), 1024 * 1024);
        assert_eq!(super::cautious_capacity::<u64>(Some(usize::MAX)), 128 * 1024);
        assert_eq!(super::cautious_capacity::<u8>(Some(7)), 7);
        assert_eq!(super::cautious_capacity::<u8>(None), 0);
    }
}
