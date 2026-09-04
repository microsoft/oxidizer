// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use serde::Serialize;
use serde::ser::{Impossible, Serializer};

#[derive(Debug)]
pub(crate) struct SerializeError;

impl fmt::Display for SerializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unsupported test serialization")
    }
}

impl std::error::Error for SerializeError {}

impl serde::ser::Error for SerializeError {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self
    }
}

pub(crate) struct ValueSerializer;

macro_rules! unsupported_scalar {
    ($($method:ident($value:ty)),+ $(,)?) => {
        $(
            fn $method(self, _value: $value) -> Result<Self::Ok, Self::Error> {
                Err(SerializeError)
            }
        )+
    };
}

impl Serializer for ValueSerializer {
    type Ok = String;
    type Error = SerializeError;
    type SerializeSeq = Impossible<String, SerializeError>;
    type SerializeTuple = Impossible<String, SerializeError>;
    type SerializeTupleStruct = Impossible<String, SerializeError>;
    type SerializeTupleVariant = Impossible<String, SerializeError>;
    type SerializeMap = Impossible<String, SerializeError>;
    type SerializeStruct = Impossible<String, SerializeError>;
    type SerializeStructVariant = Impossible<String, SerializeError>;

    unsupported_scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_bytes(&[u8]),
    );

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_string())
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        Ok(v.to_owned())
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_unit_variant(self, _name: &'static str, _variant_index: u32, _variant: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _name: &'static str, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, Self::Error> {
        Err(SerializeError)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(SerializeError)
    }
}
