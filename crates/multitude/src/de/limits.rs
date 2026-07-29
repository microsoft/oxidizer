// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Format-independent deserialization limits.

#![expect(
    clippy::renamed_function_params,
    reason = "visitor parameters consistently use value-oriented names across string and byte variants"
)]

use core::cell::Cell;
use core::fmt;
use core::marker::PhantomData;

use serde::de::{DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};

use super::{DeserializationResource, LimitExceeded};

mod deserialization_limits;
#[cfg(test)]
mod tests;

/// Resource limits for format-independent deserialization.
///
/// ```
/// use multitude::de::DeserializationLimits;
///
/// let limits = DeserializationLimits::unlimited().with_max_sequence_len(32);
/// assert_eq!(limits.max_sequence_len, 32);
/// ```
pub use deserialization_limits::DeserializationLimits;

pub(super) fn deserialize_seed_with_limits<'de, D, S>(deserializer: D, seed: S, limits: DeserializationLimits) -> Result<S::Value, D::Error>
where
    D: Deserializer<'de>,
    S: DeserializeSeed<'de>,
{
    deserialize_seed_with_limits_detailed(deserializer, seed, limits).0
}

pub(super) fn deserialize_seed_with_limits_detailed<'de, D, S>(
    deserializer: D,
    seed: S,
    limits: DeserializationLimits,
) -> (Result<S::Value, D::Error>, Option<LimitExceeded>)
where
    D: Deserializer<'de>,
    S: DeserializeSeed<'de>,
{
    let state = LimitState {
        limits,
        depth: Cell::new(0),
        limit_exceeded: Cell::new(None),
    };
    let result = seed.deserialize(LimitedDeserializer {
        inner: deserializer,
        state: &state,
    });
    (result, state.limit_exceeded.get())
}

struct LimitState {
    limits: DeserializationLimits,
    depth: Cell<usize>,
    limit_exceeded: Cell<Option<LimitExceeded>>,
}

struct DepthGuard<'a>(&'a LimitState);

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

impl LimitState {
    fn enter<E: serde::de::Error>(&self) -> Result<(), E> {
        if self.limits.max_depth == usize::MAX {
            return Ok(());
        }

        let depth = self.depth.get().saturating_add(1);
        if depth > self.limits.max_depth {
            return Err(self.reject(
                DeserializationResource::Depth,
                self.limits.max_depth,
                "deserialization nesting depth limit exceeded",
            ));
        }
        self.depth.set(depth);
        Ok(())
    }

    fn leave(&self) {
        if self.limits.max_depth != usize::MAX {
            self.depth.set(self.depth.get() - 1);
        }
    }

    fn enter_guard<E: serde::de::Error>(&self) -> Result<DepthGuard<'_>, E> {
        self.enter()?;
        Ok(DepthGuard(self))
    }

    #[cold]
    fn reject<E: serde::de::Error>(&self, resource: DeserializationResource, limit: usize, message: &'static str) -> E {
        if self.limit_exceeded.get().is_none() {
            self.limit_exceeded.set(Some(LimitExceeded::new(resource, limit)));
        }
        E::custom(message)
    }
}

struct LimitedSeed<'a, S> {
    inner: S,
    state: &'a LimitState,
}

struct RejectSeed<'a, T> {
    state: &'a LimitState,
    resource: DeserializationResource,
    limit: usize,
    message: &'static str,
    marker: PhantomData<fn() -> T>,
}

impl<'a, T> RejectSeed<'a, T> {
    fn new(state: &'a LimitState, resource: DeserializationResource, limit: usize, message: &'static str) -> Self {
        Self {
            state,
            resource,
            limit,
            message,
            marker: PhantomData,
        }
    }
}

impl<'de, T> DeserializeSeed<'de> for RejectSeed<'_, T> {
    type Value = T;

    #[cold]
    fn deserialize<D>(self, _: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(self.state.reject(self.resource, self.limit, self.message))
    }
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for LimitedSeed<'_, S> {
    type Value = S::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let _depth_guard = self.state.enter_guard::<D::Error>()?;
        self.inner.deserialize(LimitedDeserializer {
            inner: deserializer,
            state: self.state,
        })
    }
}

struct LimitedDeserializer<'a, D> {
    inner: D,
    state: &'a LimitState,
}

macro_rules! delegate_deserializer {
    ($($method:ident $(($($arg:ident: $arg_ty:ty),* $(,)?))?);+ $(;)?) => {
        $(
            fn $method<V>(self, $($($arg: $arg_ty,)*)? visitor: V) -> Result<V::Value, Self::Error>
            where
                V: Visitor<'de>,
            {
                self.inner.$method(
                    $($($arg,)*)?
                    LimitedVisitor {
                        inner: visitor,
                        state: self.state,
                    },
                )
            }
        )+
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for LimitedDeserializer<'_, D> {
    type Error = D::Error;

    delegate_deserializer! {
        deserialize_any;
        deserialize_bool;
        deserialize_i8;
        deserialize_i16;
        deserialize_i32;
        deserialize_i64;
        deserialize_i128;
        deserialize_u8;
        deserialize_u16;
        deserialize_u32;
        deserialize_u64;
        deserialize_u128;
        deserialize_f32;
        deserialize_f64;
        deserialize_char;
        deserialize_str;
        deserialize_string;
        deserialize_bytes;
        deserialize_byte_buf;
        deserialize_option;
        deserialize_unit;
        deserialize_unit_struct(name: &'static str);
        deserialize_newtype_struct(name: &'static str);
        deserialize_seq;
        deserialize_tuple(len: usize);
        deserialize_tuple_struct(name: &'static str, len: usize);
        deserialize_map;
        deserialize_struct(name: &'static str, fields: &'static [&'static str]);
        deserialize_enum(name: &'static str, variants: &'static [&'static str]);
        deserialize_identifier;
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.inner.deserialize_any(LimitedVisitor {
            inner: visitor,
            state: self.state,
        })
    }
}

struct LimitedVisitor<'a, V> {
    inner: V,
    state: &'a LimitState,
}

macro_rules! forward_visit {
    ($($method:ident($value:ident: $ty:ty));+ $(;)?) => {
        $(
            fn $method<E: serde::de::Error>(self, $value: $ty) -> Result<Self::Value, E> {
                self.inner.$method($value)
            }
        )+
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for LimitedVisitor<'_, V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(formatter)
    }

    forward_visit! {
        visit_bool(value: bool);
        visit_i8(value: i8);
        visit_i16(value: i16);
        visit_i32(value: i32);
        visit_i64(value: i64);
        visit_i128(value: i128);
        visit_u8(value: u8);
        visit_u16(value: u16);
        visit_u32(value: u32);
        visit_u64(value: u64);
        visit_u128(value: u128);
        visit_f32(value: f32);
        visit_f64(value: f64);
        visit_char(value: char);
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.check_string::<E>(value)?;
        self.inner.visit_str(value)
    }

    fn visit_borrowed_str<E: serde::de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        self.check_string::<E>(value)?;
        self.inner.visit_borrowed_str(value)
    }

    fn visit_string<E: serde::de::Error>(self, value: alloc::string::String) -> Result<Self::Value, E> {
        self.check_string::<E>(&value)?;
        self.inner.visit_string(value)
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        self.check_bytes::<E>(value)?;
        self.inner.visit_bytes(value)
    }

    fn visit_borrowed_bytes<E: serde::de::Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
        self.check_bytes::<E>(value)?;
        self.inner.visit_borrowed_bytes(value)
    }

    fn visit_byte_buf<E: serde::de::Error>(self, value: alloc::vec::Vec<u8>) -> Result<Self::Value, E> {
        self.check_bytes::<E>(&value)?;
        self.inner.visit_byte_buf(value)
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let _depth_guard = self.state.enter_guard::<D::Error>()?;
        self.inner.visit_some(LimitedDeserializer {
            inner: deserializer,
            state: self.state,
        })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let _depth_guard = self.state.enter_guard::<D::Error>()?;
        self.inner.visit_newtype_struct(LimitedDeserializer {
            inner: deserializer,
            state: self.state,
        })
    }

    fn visit_seq<S: SeqAccess<'de>>(self, seq: S) -> Result<Self::Value, S::Error> {
        self.inner.visit_seq(LimitedSeqAccess {
            inner: seq,
            state: self.state,
            count: 0,
        })
    }

    fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
        self.inner.visit_map(LimitedMapAccess {
            inner: map,
            state: self.state,
            count: 0,
        })
    }

    fn visit_enum<E: EnumAccess<'de>>(self, data: E) -> Result<Self::Value, E::Error> {
        self.inner.visit_enum(LimitedEnumAccess {
            inner: data,
            state: self.state,
        })
    }
}

impl<V> LimitedVisitor<'_, V> {
    fn check_string<E: serde::de::Error>(&self, value: &str) -> Result<(), E> {
        let limit = self.state.limits.max_string_len;
        if limit != usize::MAX && value.len() > limit {
            Err(self.state.reject(
                DeserializationResource::StringLength,
                limit,
                "deserialization string length limit exceeded",
            ))
        } else {
            Ok(())
        }
    }

    fn check_bytes<E: serde::de::Error>(&self, value: &[u8]) -> Result<(), E> {
        let limit = self.state.limits.max_bytes_len;
        if limit != usize::MAX && value.len() > limit {
            Err(self.state.reject(
                DeserializationResource::ByteStringLength,
                limit,
                "deserialization byte string length limit exceeded",
            ))
        } else {
            Ok(())
        }
    }
}

struct LimitedSeqAccess<'a, S> {
    inner: S,
    state: &'a LimitState,
    count: usize,
}

impl<'de, S: SeqAccess<'de>> SeqAccess<'de> for LimitedSeqAccess<'_, S> {
    type Error = S::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        let limit = self.state.limits.max_sequence_len;
        if limit != usize::MAX && self.count >= limit {
            return self.inner.next_element_seed(RejectSeed::new(
                self.state,
                DeserializationResource::SequenceLength,
                limit,
                "deserialization sequence length limit exceeded",
            ));
        }

        let value = self.inner.next_element_seed(LimitedSeed {
            inner: seed,
            state: self.state,
        })?;
        if limit != usize::MAX && value.is_some() {
            self.count += 1;
        }
        Ok(value)
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner
            .size_hint()
            .map(|hint| hint.min(self.state.limits.max_sequence_len.saturating_sub(self.count)))
    }
}

struct LimitedMapAccess<'a, M> {
    inner: M,
    state: &'a LimitState,
    count: usize,
}

impl<'de, M: MapAccess<'de>> MapAccess<'de> for LimitedMapAccess<'_, M> {
    type Error = M::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        let limit = self.state.limits.max_map_len;
        if limit != usize::MAX && self.count >= limit {
            return self.inner.next_key_seed(RejectSeed::new(
                self.state,
                DeserializationResource::MapLength,
                limit,
                "deserialization map length limit exceeded",
            ));
        }

        let key = self.inner.next_key_seed(LimitedSeed {
            inner: seed,
            state: self.state,
        })?;
        if limit != usize::MAX && key.is_some() {
            self.count += 1;
        }
        Ok(key)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        self.inner.next_value_seed(LimitedSeed {
            inner: seed,
            state: self.state,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner
            .size_hint()
            .map(|hint| hint.min(self.state.limits.max_map_len.saturating_sub(self.count)))
    }
}

struct LimitedEnumAccess<'a, E> {
    inner: E,
    state: &'a LimitState,
}

impl<'a, 'de, E: EnumAccess<'de>> EnumAccess<'de> for LimitedEnumAccess<'a, E> {
    type Error = E::Error;
    type Variant = LimitedVariantAccess<'a, E::Variant>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error> {
        let (value, variant) = self.inner.variant_seed(LimitedSeed {
            inner: seed,
            state: self.state,
        })?;
        Ok((
            value,
            LimitedVariantAccess {
                inner: variant,
                state: self.state,
            },
        ))
    }
}

struct LimitedVariantAccess<'a, V> {
    inner: V,
    state: &'a LimitState,
}

impl<'de, V: VariantAccess<'de>> VariantAccess<'de> for LimitedVariantAccess<'_, V> {
    type Error = V::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        self.inner.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Self::Error> {
        self.inner.newtype_variant_seed(LimitedSeed {
            inner: seed,
            state: self.state,
        })
    }

    fn tuple_variant<W: Visitor<'de>>(self, len: usize, visitor: W) -> Result<W::Value, Self::Error> {
        self.inner.tuple_variant(
            len,
            LimitedVisitor {
                inner: visitor,
                state: self.state,
            },
        )
    }

    fn struct_variant<W: Visitor<'de>>(self, fields: &'static [&'static str], visitor: W) -> Result<W::Value, Self::Error> {
        self.inner.struct_variant(
            fields,
            LimitedVisitor {
                inner: visitor,
                state: self.state,
            },
        )
    }
}
