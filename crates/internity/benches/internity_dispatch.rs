// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-count benchmarks for derived field identifier dispatch.
//! Run with `cargo bench -p internity --features serde --bench internity_dispatch`.

#![allow(missing_docs, reason = "generated Gungraun benchmark bindings")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Gungraun macro expansion"
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::prelude::*;
    use internity::LocalLexicon;
    use internity::de::DeserializeIn;
    use serde::de::{DeserializeSeed, MapAccess, Visitor, value};

    #[derive(DeserializeIn)]
    #[serde(deny_unknown_fields)]
    struct Dispatch {
        #[serde(default, alias = "old_name")]
        name: Option<u32>,
        #[serde(default)]
        field1: Option<u32>,
        #[serde(default)]
        field2: Option<u32>,
        #[serde(default)]
        field3: Option<u32>,
        #[serde(default)]
        field4: Option<u32>,
        #[serde(default)]
        field5: Option<u32>,
        #[serde(default)]
        field6: Option<u32>,
        #[serde(default)]
        field7: Option<u32>,
    }

    #[derive(Clone, Copy)]
    enum Key {
        Str(&'static str),
        Bytes(&'static [u8]),
        Index(u64),
    }

    impl<'de> serde::Deserializer<'de> for Key {
        type Error = value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            self.deserialize_identifier(visitor)
        }

        fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            match self {
                Self::Str(key) => visitor.visit_str(key),
                Self::Bytes(key) => visitor.visit_bytes(key),
                Self::Index(key) => visitor.visit_u64(key),
            }
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char string
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
            map struct enum ignored_any
        }

        fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            self.deserialize_identifier(visitor)
        }

        fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            self.deserialize_identifier(visitor)
        }
    }

    struct OneField {
        key: Option<Key>,
        value: Option<u32>,
    }

    impl<'de> MapAccess<'de> for OneField {
        type Error = value::Error;

        fn next_key_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<Option<S::Value>, Self::Error> {
            self.key.take().map(|key| seed.deserialize(key)).transpose()
        }

        fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, Self::Error> {
            let value = self
                .value
                .take()
                .ok_or_else(|| serde::de::Error::custom("value requested without a key"))?;
            seed.deserialize(value::U32Deserializer::<Self::Error>::new(value))
        }
    }

    struct Input(Key);

    impl<'de> serde::Deserializer<'de> for Input {
        type Error = value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            self.deserialize_struct("", &[], visitor)
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            _fields: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            visitor.visit_map(OneField {
                key: Some(self.0),
                value: Some(42),
            })
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
            map enum identifier ignored_any
        }
    }

    fn dispatch(key: Key) -> bool {
        let mut lexicon = LocalLexicon::new();
        match Dispatch::deserialize_in(&mut lexicon, Input(black_box(key))) {
            Ok(value) => {
                black_box((
                    value.name,
                    value.field1,
                    value.field2,
                    value.field3,
                    value.field4,
                    value.field5,
                    value.field6,
                    value.field7,
                ));
                true
            }
            Err(_) => false,
        }
    }

    #[library_benchmark]
    fn fields_str_first() -> bool {
        dispatch(Key::Str("name"))
    }

    #[library_benchmark]
    fn fields_str_last() -> bool {
        dispatch(Key::Str("field7"))
    }

    #[library_benchmark]
    fn fields_str_alias() -> bool {
        dispatch(Key::Str("old_name"))
    }

    #[library_benchmark]
    fn fields_str_unknown() -> bool {
        dispatch(Key::Str("unknown"))
    }

    #[library_benchmark]
    fn fields_bytes_first() -> bool {
        dispatch(Key::Bytes(b"name"))
    }

    #[library_benchmark]
    fn fields_bytes_last() -> bool {
        dispatch(Key::Bytes(b"field7"))
    }

    #[library_benchmark]
    fn fields_bytes_alias() -> bool {
        dispatch(Key::Bytes(b"old_name"))
    }

    #[library_benchmark]
    fn fields_bytes_unknown() -> bool {
        dispatch(Key::Bytes(b"unknown"))
    }

    #[library_benchmark]
    fn fields_numeric() -> bool {
        dispatch(Key::Index(7))
    }

    library_benchmark_group!(
        name = fields;
        benchmarks =
            fields_str_first, fields_str_last, fields_str_alias, fields_str_unknown,
            fields_bytes_first, fields_bytes_last, fields_bytes_alias, fields_bytes_unknown,
            fields_numeric
    );
}

#[cfg(target_os = "linux")]
pub use linux::fields;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = fields);
