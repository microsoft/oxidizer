// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The private field-wrapper deserializer used by request decoding.

use serde::de::value::StrDeserializer;
use serde::de::{DeserializeSeed, Deserializer, MapAccess, Visitor};

/// A [`Deserializer`] that presents `{ "<field>": <body> }` to `T` without
/// building the wrapper, deserializing `body` straight into the field's value.
pub(crate) struct FieldDeserializer<'a> {
    field: &'static str,
    body: &'a [u8],
}

impl<'a> FieldDeserializer<'a> {
    pub(crate) const fn new(field: &'static str, body: &'a [u8]) -> Self {
        Self { field, body }
    }
}

impl<'de, 'a: 'de> Deserializer<'de> for FieldDeserializer<'a> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(FieldMapAccess {
            field: Some(self.field),
            body: self.body,
        })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

/// The single-entry [`MapAccess`] backing [`FieldDeserializer`]: yields the
/// field name once, then deserializes the value from the raw JSON body.
struct FieldMapAccess<'a> {
    field: Option<&'static str>,
    body: &'a [u8],
}

impl<'de, 'a: 'de> MapAccess<'de> for FieldMapAccess<'a> {
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        match self.field.take() {
            Some(field) => seed.deserialize(StrDeserializer::new(field)).map(Some),
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        let mut de = serde_json::Deserializer::from_slice(self.body);
        let value = seed.deserialize(&mut de)?;
        // Mirror `from_slice`: reject trailing bytes after the field value.
        de.end()?;
        Ok(value)
    }
}
