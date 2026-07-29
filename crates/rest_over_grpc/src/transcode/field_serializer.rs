// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The private field-extracting serializer used by response encoding.

use core::fmt;
use std::error::Error;

use serde::Serialize;
use serde::ser::{Impossible, SerializeStruct, Serializer};
use serde_json::{Error as JsonError, Serializer as JsonSerializer};

/// Serializes one top-level struct field directly into JSON.
pub(crate) struct FieldSerializer<'a> {
    field: &'static str,
    out: &'a mut Vec<u8>,
}

impl<'a> FieldSerializer<'a> {
    pub(crate) fn new(field: &'static str, out: &'a mut Vec<u8>) -> Self {
        Self { field, out }
    }
}

/// The failure modes of [`FieldSerializer`].
#[derive(Debug)]
pub(crate) enum FieldSerError {
    Absent,
    Unsupported,
    Json(JsonError),
    Custom(String),
}

impl fmt::Display for FieldSerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("response_body field is absent from the message"),
            Self::Unsupported => f.write_str("message is not a struct"),
            Self::Json(source) => write!(f, "{source}"),
            Self::Custom(detail) => f.write_str(detail),
        }
    }
}

impl Error for FieldSerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(source) => Some(source),
            _ => None,
        }
    }
}

impl serde::ser::Error for FieldSerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Custom(msg.to_string())
    }
}

impl<'a> Serializer for FieldSerializer<'a> {
    type Ok = ();
    type Error = FieldSerError;

    type SerializeSeq = Impossible<(), FieldSerError>;
    type SerializeTuple = Impossible<(), FieldSerError>;
    type SerializeTupleStruct = Impossible<(), FieldSerError>;
    type SerializeTupleVariant = Impossible<(), FieldSerError>;
    type SerializeMap = Impossible<(), FieldSerError>;
    type SerializeStruct = FieldStruct<'a>;
    type SerializeStructVariant = Impossible<(), FieldSerError>;

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(FieldStruct {
            field: self.field,
            out: self.out,
            found: false,
        })
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _name: &'static str, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_str(self, _value: &str) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_unit_variant(self, _name: &'static str, _index: u32, _variant: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(FieldSerError::Unsupported)
    }

    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(FieldSerError::Unsupported)
    }
}

pub(crate) struct FieldStruct<'a> {
    field: &'static str,
    out: &'a mut Vec<u8>,
    found: bool,
}

impl SerializeStruct for FieldStruct<'_> {
    type Ok = ();
    type Error = FieldSerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error> {
        if !self.found && key == self.field {
            let mut serializer = JsonSerializer::new(&mut *self.out);
            value.serialize(&mut serializer).map_err(FieldSerError::Json)?;
            self.found = true;
        }
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.found { Ok(()) } else { Err(FieldSerError::Absent) }
    }
}

#[cfg(test)]
mod tests {
    use serde::ser::Error as _;

    use super::*;

    macro_rules! assert_variant {
        ($variant:pat, $method:ident ( $($arg:expr),* )) => {{
            let mut buf = Vec::new();
            let serializer = FieldSerializer::new("target", &mut buf);
            let result = serializer.$method($($arg),*).map(|_| ());
            assert!(matches!(result, Err($variant)), "unexpected result for {}", stringify!($method));
        }};
    }

    #[test]
    fn scalar_and_sequence_shapes_report_unsupported() {
        assert_variant!(FieldSerError::Unsupported, serialize_bool(true));
        assert_variant!(FieldSerError::Unsupported, serialize_i8(0));
        assert_variant!(FieldSerError::Unsupported, serialize_i16(0));
        assert_variant!(FieldSerError::Unsupported, serialize_i32(0));
        assert_variant!(FieldSerError::Unsupported, serialize_i64(0));
        assert_variant!(FieldSerError::Unsupported, serialize_i128(0));
        assert_variant!(FieldSerError::Unsupported, serialize_u8(0));
        assert_variant!(FieldSerError::Unsupported, serialize_u16(0));
        assert_variant!(FieldSerError::Unsupported, serialize_u32(0));
        assert_variant!(FieldSerError::Unsupported, serialize_u64(0));
        assert_variant!(FieldSerError::Unsupported, serialize_u128(0));
        assert_variant!(FieldSerError::Unsupported, serialize_f32(0.0));
        assert_variant!(FieldSerError::Unsupported, serialize_f64(0.0));
        assert_variant!(FieldSerError::Unsupported, serialize_char('a'));
        assert_variant!(FieldSerError::Unsupported, serialize_str("s"));
        assert_variant!(FieldSerError::Unsupported, serialize_bytes(b"b"));
        assert_variant!(FieldSerError::Unsupported, serialize_none());
        assert_variant!(FieldSerError::Unsupported, serialize_unit());
        assert_variant!(FieldSerError::Unsupported, serialize_unit_struct("U"));
        assert_variant!(FieldSerError::Unsupported, serialize_unit_variant("E", 0, "V"));
        assert_variant!(FieldSerError::Unsupported, serialize_seq(None));
        assert_variant!(FieldSerError::Unsupported, serialize_tuple(2));
        assert_variant!(FieldSerError::Unsupported, serialize_tuple_struct("T", 2));
    }

    #[test]
    fn option_and_newtype_forward_to_the_inner_value() {
        assert_variant!(FieldSerError::Unsupported, serialize_some(&0_i32));
        assert_variant!(FieldSerError::Unsupported, serialize_newtype_struct("N", &0_i32));
    }

    #[test]
    fn keyed_non_struct_shapes_report_unsupported() {
        assert_variant!(FieldSerError::Unsupported, serialize_map(None));
        assert_variant!(FieldSerError::Unsupported, serialize_newtype_variant("E", 0, "V", &0_i32));
        assert_variant!(FieldSerError::Unsupported, serialize_tuple_variant("E", 0, "V", 1));
        assert_variant!(FieldSerError::Unsupported, serialize_struct_variant("E", 0, "V", 1));
    }

    #[test]
    fn struct_extracts_only_the_selected_field() {
        #[derive(Serialize)]
        struct Msg {
            other: u32,
            target: &'static str,
            extra: bool,
        }
        let mut buf = Vec::new();
        Msg {
            other: 1,
            target: "hit",
            extra: true,
        }
        .serialize(FieldSerializer::new("target", &mut buf))
        .expect("field present");
        assert_eq!(buf, br#""hit""#);
    }

    #[test]
    fn struct_missing_the_field_is_absent() {
        #[derive(Serialize)]
        struct Msg {
            other: u32,
        }
        let mut buf = Vec::new();
        let result = Msg { other: 1 }.serialize(FieldSerializer::new("target", &mut buf));
        assert!(matches!(result, Err(FieldSerError::Absent)));
    }

    #[test]
    fn error_display_and_source_cover_every_variant() {
        assert_eq!(FieldSerError::Absent.to_string(), "response_body field is absent from the message");
        assert_eq!(FieldSerError::Unsupported.to_string(), "message is not a struct");
        assert_eq!(FieldSerError::Custom("boom".to_owned()).to_string(), "boom");

        let json_error = serde_json::from_str::<i32>("not-json").expect_err("invalid JSON");
        let wrapped = FieldSerError::Json(json_error);
        assert!(!wrapped.to_string().is_empty());

        assert!(wrapped.source().is_some());
        assert!(FieldSerError::Absent.source().is_none());

        let custom = FieldSerError::custom("via custom");
        assert!(matches!(custom, FieldSerError::Custom(detail) if detail == "via custom"));
    }
}
