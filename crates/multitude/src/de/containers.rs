// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Format-independent implementations for recursive container types.

#![expect(
    clippy::elidable_lifetime_names,
    clippy::renamed_function_params,
    clippy::use_self,
    reason = "Serde visitor names describe collection roles, and explicit deserializer lifetimes clarify recursive bounds"
)]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::cmp::Reverse;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::{fmt, ptr};

use allocator_api2::alloc::Allocator;
use serde::de::{self, Deserializer, EnumAccess, Error as _, MapAccess, SeqAccess, VariantAccess, Visitor};

use super::{Arena, DeserializeIn, DeserializeInSeed};

const fn overlong_array_length(length: usize) -> usize {
    length.saturating_add(1)
}

const fn vec_needs_reserve(length: usize, capacity: usize) -> bool {
    length == capacity
}

impl<'de, T: ?Sized, A: Allocator + Clone> DeserializeIn<'de, A> for PhantomData<T> {
    fn deserialize_in<D>(_arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PhantomDataVisitor<T: ?Sized>(PhantomData<T>);

        impl<'de, T: ?Sized> Visitor<'de> for PhantomDataVisitor<T> {
            type Value = PhantomData<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("unit")
            }

            #[cfg_attr(test, mutants::skip)] // `PhantomData<T>::default()` is also `PhantomData`
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(PhantomData)
            }
        }

        deserializer.deserialize_unit_struct("PhantomData", PhantomDataVisitor(PhantomData))
    }
}

impl<'de, T, E, A> DeserializeIn<'de, A> for Result<T, E>
where
    T: DeserializeIn<'de, A>,
    E: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        enum Field {
            Ok,
            Err,
        }

        impl<'de> serde::Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct FieldVisitor;

                impl<'de> Visitor<'de> for FieldVisitor {
                    type Value = Field;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("`Ok` or `Err`")
                    }

                    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                        match value {
                            0 => Ok(Field::Ok),
                            1 => Ok(Field::Err),
                            _ => Err(E::invalid_value(de::Unexpected::Unsigned(value), &self)),
                        }
                    }

                    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        match value {
                            "Ok" => Ok(Field::Ok),
                            "Err" => Ok(Field::Err),
                            _ => Err(E::unknown_variant(value, &["Ok", "Err"])),
                        }
                    }

                    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                        match value {
                            b"Ok" => Ok(Field::Ok),
                            b"Err" => Ok(Field::Err),
                            _ => match core::str::from_utf8(value) {
                                Ok(value) => Err(E::unknown_variant(value, &["Ok", "Err"])),
                                Err(_) => Err(E::invalid_value(de::Unexpected::Bytes(value), &self)),
                            },
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct ResultVisitor<'a, T, E, A: Allocator + Clone> {
            arena: &'a Arena<A>,
            marker: PhantomData<fn() -> Result<T, E>>,
        }

        impl<'de, T, E, A> Visitor<'de> for ResultVisitor<'_, T, E, A>
        where
            T: DeserializeIn<'de, A>,
            E: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = Result<T, E>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("enum Result")
            }

            fn visit_enum<V>(self, data: V) -> Result<Self::Value, V::Error>
            where
                V: EnumAccess<'de>,
            {
                match data.variant::<Field>()? {
                    (Field::Ok, variant) => variant.newtype_variant_seed(DeserializeInSeed::<T, A>::new(self.arena)).map(Ok),
                    (Field::Err, variant) => variant.newtype_variant_seed(DeserializeInSeed::<E, A>::new(self.arena)).map(Err),
                }
            }
        }

        deserializer.deserialize_enum(
            "Result",
            &["Ok", "Err"],
            ResultVisitor {
                arena,
                marker: PhantomData,
            },
        )
    }
}

macro_rules! tuple_impl {
    ($length:expr, $(($type:ident, $index:tt)),+ $(,)?) => {
        impl<'de, A, $($type),+> DeserializeIn<'de, A> for ($($type,)+)
        where
            A: Allocator + Clone,
            $($type: DeserializeIn<'de, A>,)+
        {
            fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct TupleVisitor<'a, A: Allocator + Clone, $($type),+> {
                    arena: &'a Arena<A>,
                    marker: PhantomData<fn() -> ($($type,)+)>,
                }

                impl<'de, A, $($type),+> Visitor<'de> for TupleVisitor<'_, A, $($type),+>
                where
                    A: Allocator + Clone,
                    $($type: DeserializeIn<'de, A>,)+
                {
                    type Value = ($($type,)+);

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(formatter, "a tuple of size {}", $length)
                    }

                    fn visit_seq<S>(self, mut sequence: S) -> Result<Self::Value, S::Error>
                    where
                        S: SeqAccess<'de>,
                    {
                        Ok((
                            $(
                                sequence
                                    .next_element_seed(DeserializeInSeed::<$type, A>::new(self.arena))?
                                    .ok_or_else(|| S::Error::invalid_length($index, &self))?,
                            )+
                        ))
                    }
                }

                deserializer.deserialize_tuple(
                    $length,
                    TupleVisitor {
                        arena,
                        marker: PhantomData,
                    },
                )
            }
        }
    };
}

tuple_impl!(1, (T0, 0));
tuple_impl!(2, (T0, 0), (T1, 1));
tuple_impl!(3, (T0, 0), (T1, 1), (T2, 2));
tuple_impl!(4, (T0, 0), (T1, 1), (T2, 2), (T3, 3));
tuple_impl!(5, (T0, 0), (T1, 1), (T2, 2), (T3, 3), (T4, 4));
tuple_impl!(6, (T0, 0), (T1, 1), (T2, 2), (T3, 3), (T4, 4), (T5, 5));
tuple_impl!(7, (T0, 0), (T1, 1), (T2, 2), (T3, 3), (T4, 4), (T5, 5), (T6, 6));
tuple_impl!(8, (T0, 0), (T1, 1), (T2, 2), (T3, 3), (T4, 4), (T5, 5), (T6, 6), (T7, 7));
tuple_impl!(9, (T0, 0), (T1, 1), (T2, 2), (T3, 3), (T4, 4), (T5, 5), (T6, 6), (T7, 7), (T8, 8));
tuple_impl!(
    10,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9)
);
tuple_impl!(
    11,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9),
    (T10, 10)
);
tuple_impl!(
    12,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9),
    (T10, 10),
    (T11, 11)
);
tuple_impl!(
    13,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9),
    (T10, 10),
    (T11, 11),
    (T12, 12)
);
tuple_impl!(
    14,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9),
    (T10, 10),
    (T11, 11),
    (T12, 12),
    (T13, 13)
);
tuple_impl!(
    15,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9),
    (T10, 10),
    (T11, 11),
    (T12, 12),
    (T13, 13),
    (T14, 14)
);
tuple_impl!(
    16,
    (T0, 0),
    (T1, 1),
    (T2, 2),
    (T3, 3),
    (T4, 4),
    (T5, 5),
    (T6, 6),
    (T7, 7),
    (T8, 8),
    (T9, 9),
    (T10, 10),
    (T11, 11),
    (T12, 12),
    (T13, 13),
    (T14, 14),
    (T15, 15)
);

impl<'de, T, A, const N: usize> DeserializeIn<'de, A> for [T; N]
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ArrayVisitor<'a, T, A: Allocator + Clone, const N: usize> {
            arena: &'a Arena<A>,
            marker: PhantomData<fn() -> T>,
        }

        struct InitGuard<T, const N: usize> {
            values: [MaybeUninit<T>; N],
            initialized: usize,
        }

        impl<T, const N: usize> Drop for InitGuard<T, N> {
            fn drop(&mut self) {
                let initialized = ptr::slice_from_raw_parts_mut(self.values.as_mut_ptr().cast::<T>(), self.initialized);
                // SAFETY: Only the prefix counted by `initialized` was written.
                // Slice drop glue continues dropping later elements if one panics.
                unsafe { ptr::drop_in_place(initialized) };
            }
        }

        impl<'de, T, A, const N: usize> Visitor<'de> for ArrayVisitor<'_, T, A, N>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = [T; N];

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "an array of length {N}")
            }

            fn visit_seq<S>(self, mut sequence: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                let mut guard = InitGuard {
                    values: [const { MaybeUninit::uninit() }; N],
                    initialized: 0,
                };

                while guard.initialized < N {
                    let index = guard.initialized;
                    let value = sequence
                        .next_element_seed(DeserializeInSeed::<T, A>::new(self.arena))?
                        .ok_or_else(|| S::Error::invalid_length(index, &self))?;
                    guard.values[index].write(value);
                    guard.initialized += 1;
                }

                if sequence.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(S::Error::invalid_length(overlong_array_length(N), &self));
                }

                guard.initialized = 0;
                let values = (&raw const guard.values).cast::<[T; N]>();
                // SAFETY: Every element was initialized above, and setting the
                // guard count to zero transferred responsibility to this array.
                Ok(unsafe { values.read() })
            }
        }

        deserializer.deserialize_tuple(
            N,
            ArrayVisitor {
                arena,
                marker: PhantomData,
            },
        )
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Vec<T>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VecVisitor<'a, T, A: Allocator + Clone> {
            arena: &'a Arena<A>,
            marker: PhantomData<fn() -> T>,
        }

        impl<'de, T, A> Visitor<'de> for VecVisitor<'_, T, A>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = Vec<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S>(self, mut sequence: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                if let Some(capacity) = sequence.size_hint() {
                    values.try_reserve(capacity).map_err(S::Error::custom)?;
                }
                while let Some(value) = sequence.next_element_seed(DeserializeInSeed::<T, A>::new(self.arena))? {
                    if vec_needs_reserve(values.len(), values.capacity()) {
                        values.try_reserve(1).map_err(S::Error::custom)?;
                    }
                    values.push(value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_seq(VecVisitor {
            arena,
            marker: PhantomData,
        })
    }
}

macro_rules! transparent_wrapper {
    ($wrapper:ident) => {
        impl<'de, T, A> DeserializeIn<'de, A> for $wrapper<T>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize_in(arena, deserializer).map($wrapper::new)
            }
        }
    };
}

transparent_wrapper!(Cell);
transparent_wrapper!(RefCell);

impl<'de, T, A> DeserializeIn<'de, A> for Reverse<T>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize_in(arena, deserializer).map(Reverse)
    }
}

impl<'de, K, V, A> DeserializeIn<'de, A> for BTreeMap<K, V>
where
    K: DeserializeIn<'de, A> + Ord,
    V: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MapVisitor<'a, K, V, A: Allocator + Clone> {
            arena: &'a Arena<A>,
            marker: PhantomData<fn() -> (K, V)>,
        }

        impl<'de, K, V, A> Visitor<'de> for MapVisitor<'_, K, V, A>
        where
            K: DeserializeIn<'de, A> + Ord,
            V: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = BTreeMap<K, V>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry_seed(
                    DeserializeInSeed::<K, A>::new(self.arena),
                    DeserializeInSeed::<V, A>::new(self.arena),
                )? {
                    values.insert(key, value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_map(MapVisitor {
            arena,
            marker: PhantomData,
        })
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for BTreeSet<T>
where
    T: DeserializeIn<'de, A> + Ord,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SetVisitor<'a, T, A: Allocator + Clone> {
            arena: &'a Arena<A>,
            marker: PhantomData<fn() -> T>,
        }

        impl<'de, T, A> Visitor<'de> for SetVisitor<'_, T, A>
        where
            T: DeserializeIn<'de, A> + Ord,
            A: Allocator + Clone,
        {
            type Value = BTreeSet<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Self::Value, S::Error> {
                let mut values = BTreeSet::new();
                while let Some(value) = sequence.next_element_seed(DeserializeInSeed::<T, A>::new(self.arena))? {
                    values.insert(value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_seq(SetVisitor {
            arena,
            marker: PhantomData,
        })
    }
}

// An arena-backed hashbrown collection contains `&Arena<A>` as its allocator.
// `DeserializeIn::deserialize_in` does not tie the arena borrow to `Self`, so
// such an implementation cannot return that collection soundly. It can be
// added if the trait gains an arena lifetime parameter.

#[cfg(test)]
mod tests;
