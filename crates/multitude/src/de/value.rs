// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-owned, format-independent Serde values.
//!
//! [`Value`] captures arbitrary Serde input when the concrete destination type
//! is not known in advance. Strings, byte strings, sequences, nested values,
//! enum payloads, and map entries use arena-backed ownership. [`Number`]
//! retains the exact numeric width and category supplied by the deserializer.
//! [`Map`] preserves source order and duplicate keys instead of imposing map
//! semantics.
//!
//! A value can be inspected with its typed accessors, serialized into another
//! format, or used by reference as a Serde deserializer to replay the captured
//! data into a concrete type. [`Value::get`] returns the first matching
//! string-keyed entry; [`Value::get_all`] exposes duplicates.
//!
//! Deserialization uses [`super::DeserializeIn`] and therefore allocates owned
//! data in the supplied [`Arena`]. Limits configured through
//! [`super::DeserializationLimits`] apply recursively while the value is built.
//! Malformed input, allocation failure, type mismatches, and limit violations
//! use the error type produced by the source deserializer.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "serde_json")]
//! # fn main() -> Result<(), serde_json::Error> {
//! use multitude::Arena;
//! use multitude::de::{Number, Value};
//!
//! let arena = Arena::new();
//! let value: multitude::Box<Value> =
//!     arena.deserialize_json(br#"{"id":7,"label":"first","label":"second"}"#)?;
//! assert_eq!(
//!     value.get("id").and_then(Value::as_number),
//!     Some(&Number::U64(7))
//! );
//! assert_eq!(value.get_all("label").count(), 2);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "serde_json"))]
//! # fn main() {}
//! ```

#![expect(
    clippy::renamed_function_params,
    clippy::use_self,
    reason = "visitor names mirror data-model terminology, and explicit Value names make recursive public variants clearer"
)]

use core::fmt;

use allocator_api2::alloc::Allocator;
use serde::de::{self, DeserializeSeed, EnumAccess, Error as _, IntoDeserializer, MapAccess, SeqAccess, VariantAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};

use super::{DeserializeIn, DeserializeInSeed};
use crate::Arena;

mod dynamic_value;
mod entry;
mod enum_value;
mod map;
mod number;
#[cfg(test)]
mod tests;

/// An arena-owned dynamic Serde value.
///
/// ```
/// use multitude::de::Value;
///
/// let value: Value = Value::Bool(true);
/// assert_eq!(value.as_bool(), Some(true));
/// ```
pub use dynamic_value::Value;
/// An ordered map entry.
///
/// ```
/// use multitude::de::{Entry, Value};
///
/// let entry: Entry = Entry {
///     key: Value::Bool(false),
///     value: Value::Bool(true),
/// };
/// assert!(matches!(entry.value, Value::Bool(true)));
/// ```
pub use entry::Entry;
/// The payload of an explicitly represented enum.
///
/// ```
/// use multitude::de::EnumValue;
///
/// let value: EnumValue = EnumValue::Unit;
/// assert!(matches!(value, EnumValue::Unit));
/// ```
pub use enum_value::EnumValue;
/// An ordered map that preserves duplicate keys.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::{Entry, Map, Value};
///
/// # fn main() -> Result<(), multitude::AllocError> {
/// let arena = Arena::new();
/// let map: Map = arena.try_alloc_slice_fill_iter_box([Entry {
///     key: Value::Bool(false),
///     value: Value::Bool(true),
/// }])?;
/// assert_eq!(map.len(), 1);
/// # Ok(())
/// # }
/// ```
pub use map::Map;
/// A number retaining the category supplied by Serde.
///
/// ```
/// use multitude::de::Number;
///
/// let number = Number::I32(-4);
/// assert_eq!(number, Number::I32(-4));
/// ```
pub use number::Number;

impl<A: Allocator + Clone> Value<A> {
    /// Return whether this value represents unit or an absent option.
    ///
    /// ```
    /// use multitude::de::Value;
    ///
    /// let value: Value = Value::Unit;
    /// assert!(value.is_null());
    /// ```
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Unit | Self::None)
    }

    /// Return the boolean, if this is a boolean.
    ///
    /// ```
    /// use multitude::de::Value;
    ///
    /// let value: Value = Value::Bool(true);
    /// assert_eq!(value.as_bool(), Some(true));
    /// ```
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        if let Self::Bool(value) = self { Some(*value) } else { None }
    }

    /// Return the number, if this is a number.
    ///
    /// ```
    /// use multitude::de::{Number, Value};
    ///
    /// let value: Value = Value::Number(Number::U32(8));
    /// assert_eq!(value.as_number(), Some(&Number::U32(8)));
    /// ```
    #[must_use]
    pub const fn as_number(&self) -> Option<&Number> {
        if let Self::Number(value) = self { Some(value) } else { None }
    }

    /// Return the string, if this is a string.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::Value;
    ///
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = Arena::new();
    /// let value: Value = Value::String(arena.try_alloc_str_box("text")?);
    /// assert_eq!(value.as_str(), Some("text"));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Return the bytes, if this is a byte string.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::Value;
    ///
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = Arena::new();
    /// let value: Value = Value::Bytes(arena.try_alloc_slice_copy_box([1_u8, 2])?);
    /// assert_eq!(value.as_bytes(), Some(&[1, 2][..]));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(value) => Some(value),
            _ => None,
        }
    }

    /// Return the elements, if this is a sequence.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::Value;
    ///
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = Arena::new();
    /// let values = arena.try_alloc_slice_fill_iter_box([Value::Bool(true)])?;
    /// let value: Value = Value::Sequence(values);
    /// assert_eq!(value.as_sequence().map(<[_]>::len), Some(1));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn as_sequence(&self) -> Option<&[Self]> {
        match self {
            Self::Sequence(value) => Some(value),
            _ => None,
        }
    }

    /// Return the entries, if this is a map.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::{Entry, Value};
    ///
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = Arena::new();
    /// let entries = arena.try_alloc_slice_fill_iter_box([Entry {
    ///     key: Value::Bool(false),
    ///     value: Value::Bool(true),
    /// }])?;
    /// let value: Value = Value::Map(entries);
    /// assert_eq!(value.as_map().map(<[_]>::len), Some(1));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn as_map(&self) -> Option<&[Entry<A>]> {
        match self {
            Self::Map(value) => Some(value),
            _ => None,
        }
    }

    /// Return the first string-keyed entry named `key`.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::{Entry, Value};
    ///
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = Arena::new();
    /// let entries = arena.try_alloc_slice_fill_iter_box([Entry {
    ///     key: Value::String(arena.try_alloc_str_box("enabled")?),
    ///     value: Value::Bool(true),
    /// }])?;
    /// let value: Value = Value::Map(entries);
    /// assert_eq!(value.get("enabled").and_then(Value::as_bool), Some(true));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_map()?
            .iter()
            .find_map(|entry| (entry.key.as_str() == Some(key)).then_some(&entry.value))
    }

    /// Iterate over all string-keyed entries named `key`, including duplicates.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::{Entry, Value};
    ///
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// let arena = Arena::new();
    /// let entries = arena.try_alloc_slice_fill_iter_box([
    ///     Entry {
    ///         key: Value::String(arena.try_alloc_str_box("tag")?),
    ///         value: Value::Bool(true),
    ///     },
    ///     Entry {
    ///         key: Value::String(arena.try_alloc_str_box("tag")?),
    ///         value: Value::Bool(false),
    ///     },
    /// ])?;
    /// let value: Value = Value::Map(entries);
    /// assert_eq!(value.get_all("tag").count(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Self> + 'a {
        self.as_map()
            .into_iter()
            .flatten()
            .filter_map(move |entry| (entry.key.as_str() == Some(key)).then_some(&entry.value))
    }
}

macro_rules! number_visits {
    ($($visit:ident, $ty:ty, $variant:ident);+ $(;)?) => {$(
        fn $visit<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
            Ok(Value::Number(Number::$variant(value)))
        }
    )+};
}

struct ValueVisitor<'a, A: Allocator + Clone> {
    arena: &'a Arena<A>,
}

impl<'de, A: Allocator + Clone> Visitor<'de> for ValueVisitor<'_, A> {
    type Value = Value<A>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any Serde value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Value::Unit)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Value::None)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let value = Value::deserialize_in(self.arena, deserializer)?;
        self.arena.try_alloc_box(value).map(Value::Some).map_err(D::Error::custom)
    }

    fn visit_newtype_struct<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let value = Value::deserialize_in(self.arena, deserializer)?;
        self.arena.try_alloc_box(value).map(Value::Newtype).map_err(D::Error::custom)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    number_visits! {
        visit_i8, i8, I8; visit_i16, i16, I16; visit_i32, i32, I32;
        visit_i64, i64, I64; visit_i128, i128, I128;
        visit_u8, u8, U8; visit_u16, u16, U16; visit_u32, u32, U32;
        visit_u64, u64, U64; visit_u128, u128, U128;
        visit_f32, f32, F32; visit_f64, f64, F64;
    }

    fn visit_char<E: de::Error>(self, value: char) -> Result<Self::Value, E> {
        Ok(Value::Char(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.arena.try_alloc_str_box(value).map(Value::String).map_err(E::custom)
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        self.visit_str(value)
    }

    fn visit_string<E: de::Error>(self, value: alloc::string::String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        self.arena
            .try_alloc_vec_with_capacity(value.len())
            .and_then(|mut result| {
                result.try_extend_from_slice(value)?;
                result.try_into_boxed_slice()
            })
            .map(Value::Bytes)
            .map_err(E::custom)
    }

    fn visit_borrowed_bytes<E: de::Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
        self.visit_bytes(value)
    }

    fn visit_byte_buf<E: de::Error>(self, value: alloc::vec::Vec<u8>) -> Result<Self::Value, E> {
        self.visit_bytes(&value)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Self::Value, S::Error> {
        let mut values = self
            .arena
            .try_alloc_vec_with_capacity(sequence.size_hint().unwrap_or(0))
            .map_err(S::Error::custom)?;
        while let Some(value) = sequence.next_element_seed(DeserializeInSeed::<Value<A>, A>::new(self.arena))? {
            values.try_push(value).map_err(S::Error::custom)?;
        }
        values.try_into_boxed_slice().map(Value::Sequence).map_err(S::Error::custom)
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut entries = self
            .arena
            .try_alloc_vec_with_capacity(map.size_hint().unwrap_or(0))
            .map_err(M::Error::custom)?;
        while let Some(key) = map.next_key_seed(DeserializeInSeed::<Value<A>, A>::new(self.arena))? {
            let value = map.next_value_seed(DeserializeInSeed::<Value<A>, A>::new(self.arena))?;
            entries.try_push(Entry { key, value }).map_err(M::Error::custom)?;
        }
        entries.try_into_boxed_slice().map(Value::Map).map_err(M::Error::custom)
    }

    fn visit_enum<E: EnumAccess<'de>>(self, data: E) -> Result<Self::Value, E::Error> {
        let _ = data;
        Err(E::Error::custom(
            "an opaque EnumAccess cannot reveal its variant shape; use an externally tagged map",
        ))
    }
}

impl<'de, A: Allocator + Clone> DeserializeIn<'de, A> for Value<A> {
    fn deserialize_in<D: de::Deserializer<'de>>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor { arena })
    }
}

impl serde::Serialize for Number {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        macro_rules! serialize_number {
            ($($variant:ident => $method:ident),+ $(,)?) => {
                match self { $(Self::$variant(value) => serializer.$method(*value),)+ }
            };
        }
        serialize_number! {
            I8 => serialize_i8, I16 => serialize_i16, I32 => serialize_i32,
            I64 => serialize_i64, I128 => serialize_i128,
            U8 => serialize_u8, U16 => serialize_u16,
            U32 => serialize_u32, U64 => serialize_u64, U128 => serialize_u128,
            F32 => serialize_f32, F64 => serialize_f64,
        }
    }
}

enum EnumPayloadRef<'a, A: Allocator + Clone> {
    Newtype(&'a Value<A>),
    Tuple(&'a [Value<A>]),
    Struct(&'a [Entry<A>]),
}

impl<A: Allocator + Clone> serde::Serialize for EnumPayloadRef<'_, A> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Newtype(value) => value.serialize(serializer),
            Self::Tuple(values) => values.serialize(serializer),
            Self::Struct(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for entry in *entries {
                    map.serialize_entry(&entry.key, &entry.value)?;
                }
                map.end()
            }
        }
    }
}

fn serialize_enum_payload<S, A>(serializer: S, variant: &str, payload: &EnumPayloadRef<'_, A>) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    A: Allocator + Clone,
{
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(variant, &payload)?;
    map.end()
}

impl<A: Allocator + Clone> serde::Serialize for Value<A> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unit => serializer.serialize_unit(),
            Self::None => serializer.serialize_none(),
            Self::Some(value) => serializer.serialize_some(&**value),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Number(value) => value.serialize(serializer),
            Self::Char(value) => serializer.serialize_char(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Bytes(value) => serializer.serialize_bytes(value),
            Self::Newtype(value) => serializer.serialize_newtype_struct("Value", &**value),
            Self::Sequence(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values.iter() {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }
            Self::Map(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for entry in entries.iter() {
                    map.serialize_entry(&entry.key, &entry.value)?;
                }
                map.end()
            }
            Self::Enum {
                variant,
                value: EnumValue::Unit,
            } => serializer.serialize_str(variant),
            Self::Enum {
                variant,
                value: EnumValue::Newtype(value),
            } => serialize_enum_payload(serializer, variant, &EnumPayloadRef::Newtype(value)),
            Self::Enum {
                variant,
                value: EnumValue::Tuple(values),
            } => serialize_enum_payload(serializer, variant, &EnumPayloadRef::Tuple(values)),
            Self::Enum {
                variant,
                value: EnumValue::Struct(entries),
            } => serialize_enum_payload(serializer, variant, &EnumPayloadRef::Struct(entries)),
        }
    }
}

type ReplayError = de::value::Error;

fn type_error<T>(actual: &str, expected: &str) -> Result<T, ReplayError> {
    Err(de::Error::custom(alloc::format!("invalid type {actual}, expected {expected}")))
}

macro_rules! deserialize_number {
    ($name:ident, $expected:literal) => {
        fn $name<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            match self {
                Value::Number(_) => self.deserialize_any(visitor),
                _ => type_error("non-number", $expected),
            }
        }
    };
}

impl<'de, A: Allocator + Clone> de::Deserializer<'de> for &'de Value<A> {
    type Error = ReplayError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Unit => visitor.visit_unit(),
            Value::None => visitor.visit_none(),
            Value::Some(value) => visitor.visit_some(&**value),
            Value::Bool(value) => visitor.visit_bool(*value),
            Value::Number(number) => match number {
                Number::I8(v) => visitor.visit_i8(*v),
                Number::I16(v) => visitor.visit_i16(*v),
                Number::I32(v) => visitor.visit_i32(*v),
                Number::I64(v) => visitor.visit_i64(*v),
                Number::I128(v) => visitor.visit_i128(*v),
                Number::U8(v) => visitor.visit_u8(*v),
                Number::U16(v) => visitor.visit_u16(*v),
                Number::U32(v) => visitor.visit_u32(*v),
                Number::U64(v) => visitor.visit_u64(*v),
                Number::U128(v) => visitor.visit_u128(*v),
                Number::F32(v) => visitor.visit_f32(*v),
                Number::F64(v) => visitor.visit_f64(*v),
            },
            Value::Char(value) => visitor.visit_char(*value),
            Value::String(value) => visitor.visit_borrowed_str(value),
            Value::Bytes(value) => visitor.visit_borrowed_bytes(value),
            Value::Newtype(value) => visitor.visit_newtype_struct(&**value),
            Value::Sequence(values) => visitor.visit_seq(SeqReplay::new(values)),
            Value::Map(entries) => visitor.visit_map(MapReplay::new(entries)),
            Value::Enum { .. } => visitor.visit_enum(EnumReplay::explicit(self)),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Bool(value) => visitor.visit_bool(*value),
            _ => type_error("non-boolean", "a boolean"),
        }
    }

    deserialize_number!(deserialize_i8, "i8");
    deserialize_number!(deserialize_i16, "i16");
    deserialize_number!(deserialize_i32, "i32");
    deserialize_number!(deserialize_i64, "i64");
    deserialize_number!(deserialize_i128, "i128");
    deserialize_number!(deserialize_u8, "u8");
    deserialize_number!(deserialize_u16, "u16");
    deserialize_number!(deserialize_u32, "u32");
    deserialize_number!(deserialize_u64, "u64");
    deserialize_number!(deserialize_u128, "u128");
    deserialize_number!(deserialize_f32, "f32");
    deserialize_number!(deserialize_f64, "f64");

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Char(value) => visitor.visit_char(*value),
            Value::String(value) => visitor.visit_borrowed_str(value),
            _ => type_error("non-character", "a character"),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::String(value) => visitor.visit_borrowed_str(value),
            _ => type_error("non-string", "a string"),
        }
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Bytes(value) => visitor.visit_borrowed_bytes(value),
            _ => type_error("non-bytes", "bytes"),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Unit | Value::None => visitor.visit_none(),
            Value::Some(value) => visitor.visit_some(&**value),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Unit => visitor.visit_unit(),
            _ => type_error("non-unit", "unit"),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Newtype(value) => visitor.visit_newtype_struct(&**value),
            _ => visitor.visit_newtype_struct(self),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Sequence(values) => visitor.visit_seq(SeqReplay::new(values)),
            _ => type_error("non-sequence", "a sequence"),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Sequence(values) if values.len() == len => visitor.visit_seq(SeqReplay::new(values)),
            Value::Sequence(_) => type_error("sequence of wrong length", "a tuple"),
            _ => type_error("non-sequence", "a tuple"),
        }
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _name: &'static str, len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Map(entries) => visitor.visit_map(MapReplay::new(entries)),
            _ => type_error("non-map", "a map"),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Value::Map(entries) => visitor.visit_map(MapReplay::new(entries)),
            Value::Sequence(values) => visitor.visit_seq(SeqReplay::new(values)),
            _ => type_error("invalid struct representation", "a map or sequence"),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Value::Enum { .. } => visitor.visit_enum(EnumReplay::explicit(self)),
            Value::String(variant) => visitor.visit_enum(EnumReplay::<A>::unit(variant)),
            Value::Map(entries) if entries.len() == 1 => visitor.visit_enum(EnumReplay::external(&entries[0].key, &entries[0].value)),
            _ => type_error("invalid enum representation", "an externally tagged enum"),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }
}

struct SeqReplay<'de, A: Allocator + Clone> {
    iter: core::slice::Iter<'de, Value<A>>,
}

impl<'de, A: Allocator + Clone> SeqReplay<'de, A> {
    fn new(values: &'de [Value<A>]) -> Self {
        Self { iter: values.iter() }
    }
}

impl<'de, A: Allocator + Clone> SeqAccess<'de> for SeqReplay<'de, A> {
    type Error = ReplayError;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        self.iter.next().map(|value| seed.deserialize(value)).transpose()
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct MapReplay<'de, A: Allocator + Clone> {
    iter: core::slice::Iter<'de, Entry<A>>,
    value: Option<&'de Value<A>>,
}

impl<'de, A: Allocator + Clone> MapReplay<'de, A> {
    fn new(entries: &'de [Entry<A>]) -> Self {
        Self {
            iter: entries.iter(),
            value: None,
        }
    }
}

impl<'de, A: Allocator + Clone> MapAccess<'de> for MapReplay<'de, A> {
    type Error = ReplayError;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        let Some(entry) = self.iter.next() else { return Ok(None) };
        self.value = Some(&entry.value);
        seed.deserialize(&entry.key).map(Some)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        let value = self.value.take().ok_or_else(|| de::Error::custom("value requested before key"))?;
        seed.deserialize(value)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

enum EnumPayload<'de, A: Allocator + Clone> {
    Unit,
    Value(&'de Value<A>),
    Explicit(&'de EnumValue<A>),
}

#[derive(Clone, Copy)]
enum EnumTag<'de, A: Allocator + Clone> {
    String(&'de str),
    Value(&'de Value<A>),
}

struct EnumReplay<'de, A: Allocator + Clone> {
    variant: EnumTag<'de, A>,
    payload: EnumPayload<'de, A>,
}

impl<'de, A: Allocator + Clone> EnumReplay<'de, A> {
    fn explicit(value: &'de Value<A>) -> Self {
        let Value::Enum { variant, value } = value else {
            unreachable!("EnumReplay::explicit requires a Value::Enum input")
        };
        Self {
            variant: EnumTag::String(variant),
            payload: EnumPayload::Explicit(value),
        }
    }
    fn unit(variant: &'de str) -> Self {
        Self {
            variant: EnumTag::String(variant),
            payload: EnumPayload::Unit,
        }
    }
    fn external(variant: &'de Value<A>, value: &'de Value<A>) -> Self {
        Self {
            variant: EnumTag::Value(variant),
            payload: EnumPayload::Value(value),
        }
    }
}

impl<'de, A: Allocator + Clone> EnumAccess<'de> for EnumReplay<'de, A> {
    type Error = ReplayError;
    type Variant = Self;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = match self.variant {
            EnumTag::String(variant) => seed.deserialize(variant.into_deserializer())?,
            EnumTag::Value(variant) => seed.deserialize(variant)?,
        };
        Ok((variant, self))
    }
}

impl<'de, A: Allocator + Clone> VariantAccess<'de> for EnumReplay<'de, A> {
    type Error = ReplayError;
    fn unit_variant(self) -> Result<(), Self::Error> {
        match self.payload {
            EnumPayload::Unit | EnumPayload::Explicit(EnumValue::Unit) | EnumPayload::Value(Value::Unit) => Ok(()),
            _ => type_error("non-unit enum payload", "a unit variant"),
        }
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Self::Error> {
        match self.payload {
            EnumPayload::Value(value) => seed.deserialize(value),
            EnumPayload::Explicit(EnumValue::Newtype(value)) => seed.deserialize(&**value),
            _ => type_error("invalid enum payload", "a newtype variant"),
        }
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        match self.payload {
            EnumPayload::Value(value) => de::Deserializer::deserialize_tuple(value, len, visitor),
            EnumPayload::Explicit(EnumValue::Tuple(values)) if values.len() == len => visitor.visit_seq(SeqReplay::new(values)),
            _ => type_error("invalid enum payload", "a tuple variant"),
        }
    }
    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value, Self::Error> {
        match self.payload {
            EnumPayload::Value(value) => de::Deserializer::deserialize_struct(value, "", fields, visitor),
            EnumPayload::Explicit(EnumValue::Struct(entries)) => visitor.visit_map(MapReplay::new(entries)),
            _ => type_error("invalid enum payload", "a struct variant"),
        }
    }
}
