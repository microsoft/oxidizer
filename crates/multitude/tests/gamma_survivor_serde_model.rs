// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact Serde model callbacks and recoverable serializer failures.

#![cfg(feature = "serde")]

use std::cell::RefCell;
use std::rc::Rc;

use multitude::Arena;
use multitude::de::{DeserializeIn, Entry, EnumValue, Value};
use serde::Serialize;
use serde::de::value::Error;
use serde::de::{DeserializeSeed, Deserializer, EnumAccess, VariantAccess, Visitor};
use serde::ser::{Impossible, SerializeMap, SerializeSeq, Serializer};

struct ModelInput(u64);

impl<'de> Deserializer<'de> for ModelInput {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_enum(ModelAccess(self.0))
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, Error> {
        assert_eq!(name, "PhantomData");
        visitor.visit_unit()
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        assert_eq!(name, "Result");
        assert_eq!(variants, ["Ok", "Err"]);
        visitor.visit_enum(ModelAccess(self.0))
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit newtype_struct seq tuple tuple_struct map
        struct identifier ignored_any
    }
}

struct ModelAccess(u64);

impl<'de> EnumAccess<'de> for ModelAccess {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<(T::Value, Self), Error> {
        let value = seed.deserialize(serde::de::value::U64Deserializer::<Error>::new(self.0))?;
        Ok((value, self))
    }
}

impl<'de> VariantAccess<'de> for ModelAccess {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        Err(serde::de::Error::custom("unexpected unit"))
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(serde::de::value::U8Deserializer::<Error>::new(7))
    }
    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _visitor: V) -> Result<V::Value, Error> {
        Err(serde::de::Error::custom("unexpected tuple"))
    }
    fn struct_variant<V: Visitor<'de>>(self, _fields: &'static [&'static str], _visitor: V) -> Result<V::Value, Error> {
        Err(serde::de::Error::custom("unexpected struct"))
    }
}

struct WrappedInput(bool);

impl<'de> Deserializer<'de> for WrappedInput {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.0 {
            visitor.visit_some(ModelInput(0))
        } else {
            visitor.visit_newtype_struct(ModelInput(0))
        }
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

#[test]
fn gamma_survivor_deserializer_model_names_ordinals_and_opaque_enum_errors() {
    let arena = Arena::new();
    assert_eq!(
        core::marker::PhantomData::<u8>::deserialize_in(&arena, ModelInput(0)).unwrap(),
        core::marker::PhantomData
    );
    assert_eq!(Result::<u8, u8>::deserialize_in(&arena, ModelInput(0)).unwrap(), Ok(7));
    assert_eq!(Result::<u8, u8>::deserialize_in(&arena, ModelInput(1)).unwrap(), Err(7));
    for number in [2_u64, 3, u64::MAX] {
        let error = Result::<u8, u8>::deserialize_in(&arena, ModelInput(number)).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("invalid value: integer `{number}`, expected `Ok` or `Err`")
        );
    }
    let expected = "an opaque EnumAccess cannot reveal its variant shape; use an externally tagged map";
    assert_eq!(Value::deserialize_in(&arena, ModelInput(0)).unwrap_err().to_string(), expected);
    for some in [false, true] {
        assert_eq!(Value::deserialize_in(&arena, WrappedInput(some)).unwrap_err().to_string(), expected);
    }
}

#[derive(Clone)]
struct Recording {
    events: Rc<RefCell<Vec<String>>>,
    reject_at: Option<usize>,
}

impl Recording {
    fn event(&self, event: String) -> Result<(), Error> {
        let mut events = self.events.borrow_mut();
        let index = events.len();
        events.push(event);
        if self.reject_at == Some(index) {
            return Err(serde::ser::Error::custom("gamma serialization failure"));
        }
        Ok(())
    }
}

macro_rules! record_scalars {
    ($($method:ident($value:ident: $ty:ty) => $label:literal),+ $(,)?) => {
        $(fn $method(self, $value: $ty) -> Result<(), Error> {
            self.event(format!(concat!($label, ":{}"), $value))
        })+
    };
}

impl Serializer for Recording {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Impossible<(), Error>;
    type SerializeTupleStruct = Impossible<(), Error>;
    type SerializeTupleVariant = Impossible<(), Error>;
    type SerializeMap = Self;
    type SerializeStruct = Impossible<(), Error>;
    type SerializeStructVariant = Impossible<(), Error>;

    record_scalars! {
        serialize_bool(value: bool) => "bool",
        serialize_i8(value: i8) => "i8",
        serialize_i16(value: i16) => "i16",
        serialize_i32(value: i32) => "i32",
        serialize_i64(value: i64) => "i64",
        serialize_i128(value: i128) => "i128",
        serialize_u8(value: u8) => "u8",
        serialize_u16(value: u16) => "u16",
        serialize_u32(value: u32) => "u32",
        serialize_u64(value: u64) => "u64",
        serialize_u128(value: u128) => "u128",
        serialize_f32(value: f32) => "f32",
        serialize_f64(value: f64) => "f64",
        serialize_char(value: char) => "char",
        serialize_str(value: &str) => "str",
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<(), Error> {
        self.event(format!("bytes:{v:?}"))
    }
    fn serialize_none(self) -> Result<(), Error> {
        self.event("none".into())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Error> {
        self.event("some".into())?;
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Error> {
        self.event("unit".into())
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<(), Error> {
        self.event(format!("unit-struct:{name}"))
    }
    fn serialize_unit_variant(self, name: &'static str, variant_index: u32, variant: &'static str) -> Result<(), Error> {
        self.event(format!("unit-variant:{name}:{variant_index}:{variant}"))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, name: &'static str, value: &T) -> Result<(), Error> {
        self.event(format!("newtype:{name}"))?;
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.event(format!("newtype-variant:{name}:{variant_index}:{variant}"))?;
        value.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self, Error> {
        self.event(format!("seq:{len:?}"))?;
        Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, Error> {
        self.event(format!("map:{len:?}"))?;
        Ok(self)
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        Err(serde::ser::Error::custom("unexpected tuple"))
    }
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeTupleStruct, Error> {
        Err(serde::ser::Error::custom("unexpected tuple struct"))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        Err(serde::ser::Error::custom("unexpected tuple variant"))
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, Error> {
        Err(serde::ser::Error::custom("unexpected struct"))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        Err(serde::ser::Error::custom("unexpected struct variant"))
    }
}

impl SerializeSeq for Recording {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.event("element".into())?;
        value.serialize(self.clone())
    }
    fn end(self) -> Result<(), Error> {
        self.event("seq-end".into())
    }
}

impl SerializeMap for Recording {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        self.event("key".into())?;
        key.serialize(self.clone())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.event("value".into())?;
        value.serialize(self.clone())
    }
    fn serialize_entry<K: ?Sized + Serialize, V: ?Sized + Serialize>(&mut self, key: &K, value: &V) -> Result<(), Error> {
        self.event("entry".into())?;
        key.serialize(self.clone())?;
        value.serialize(self.clone())
    }
    fn end(self) -> Result<(), Error> {
        self.event("map-end".into())
    }
}

#[test]
fn gamma_survivor_serde_exact_model_names_lengths_and_all_error_stages() {
    let arena = Arena::new();
    let entries = || {
        arena.alloc_slice_fill_iter_box(vec![Entry {
            key: Value::String(arena.alloc_str_box("x")),
            value: Value::Bool(true),
        }])
    };
    let cases = [
        (
            Value::Newtype(arena.alloc_box(Value::Bool(true))),
            vec!["newtype:Value", "bool:true"],
        ),
        (
            Value::Sequence(arena.alloc_slice_fill_iter_box(vec![Value::Bool(false), Value::Unit])),
            vec!["seq:Some(2)", "element", "bool:false", "element", "unit", "seq-end"],
        ),
        (Value::Map(entries()), vec!["map:Some(1)", "entry", "str:x", "bool:true", "map-end"]),
        (
            Value::Enum {
                variant: arena.alloc_str_box("New"),
                value: EnumValue::Newtype(arena.alloc_box(Value::Bool(true))),
            },
            vec!["map:Some(1)", "entry", "str:New", "bool:true", "map-end"],
        ),
        (
            Value::Enum {
                variant: arena.alloc_str_box("Tuple"),
                value: EnumValue::Tuple(arena.alloc_slice_fill_iter_box(vec![Value::Unit])),
            },
            vec![
                "map:Some(1)",
                "entry",
                "str:Tuple",
                "seq:Some(1)",
                "element",
                "unit",
                "seq-end",
                "map-end",
            ],
        ),
        (
            Value::Enum {
                variant: arena.alloc_str_box("Record"),
                value: EnumValue::Struct(entries()),
            },
            vec![
                "map:Some(1)",
                "entry",
                "str:Record",
                "map:Some(1)",
                "entry",
                "str:x",
                "bool:true",
                "map-end",
                "map-end",
            ],
        ),
    ];
    for (value, expected) in cases {
        let events = Rc::new(RefCell::new(Vec::new()));
        value
            .serialize(Recording {
                events: Rc::clone(&events),
                reject_at: None,
            })
            .unwrap();
        assert_eq!(*events.borrow(), expected);
        for reject_at in 0..expected.len() {
            let events = Rc::new(RefCell::new(Vec::new()));
            let error = value
                .serialize(Recording {
                    events: Rc::clone(&events),
                    reject_at: Some(reject_at),
                })
                .unwrap_err();
            assert_eq!(error.to_string(), "gamma serialization failure");
            assert_eq!(*events.borrow(), expected[..=reject_at]);
        }
    }
}
