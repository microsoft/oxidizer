// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::assertions_on_result_states,
    reason = "tests deliberately exercise success and error branches"
)]
#![expect(clippy::items_after_statements, reason = "test-only types are clearer beside their use")]
#![expect(clippy::too_many_lines, reason = "coverage test exercises the complete deserializer dispatch")]

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};
use core::fmt as core_fmt;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer as _, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, VariantAccess, Visitor};

use super::{Entry, EnumReplay, EnumValue, Map, MapReplay, Number, SeqReplay, Value, ValueVisitor};
use crate::Arena;
use crate::de::DeserializeIn;

fn array(arena: &Arena, values: Vec<Value>) -> crate::Box<[Value], allocator_api2::alloc::Global> {
    arena.alloc_slice_fill_iter_box(values)
}

fn map(arena: &Arena, entries: Vec<Entry>) -> Map {
    arena.alloc_slice_fill_iter_box(entries)
}

fn string(arena: &Arena, value: &str) -> crate::Box<str> {
    arena.alloc_str_box(value)
}

fn boxed(arena: &Arena, value: Value) -> crate::Box<Value> {
    arena.alloc_box(value)
}

#[test]
#[should_panic(expected = "EnumReplay::explicit requires a Value::Enum input")]
fn enum_replay_explicit_rejects_non_enum_value() {
    let value: Value = Value::Unit;
    let _ = EnumReplay::explicit(&value);
}

#[test]
fn accessors_cover_every_result_path() {
    let arena = Arena::new();
    let unit: Value = Value::Unit;
    let none: Value = Value::None;
    let boolean: Value = Value::Bool(true);
    let number: Value = Value::Number(Number::I8(7));
    let text = Value::String(string(&arena, "key"));
    let bytes = Value::Bytes(arena.alloc_slice_copy_box([1_u8, 2]));
    let values = Value::Sequence(array(&arena, vec![Value::Char('x')]));
    let entries = Value::Map(map(
        &arena,
        vec![
            Entry {
                key: Value::String(string(&arena, "key")),
                value: Value::Number(Number::U8(1)),
            },
            Entry {
                key: Value::Bool(false),
                value: Value::Unit,
            },
            Entry {
                key: Value::String(string(&arena, "key")),
                value: Value::Number(Number::U8(2)),
            },
        ],
    ));

    assert!(unit.is_null());
    assert!(none.is_null());
    assert!(!boolean.is_null());
    assert_eq!(boolean.as_bool(), Some(true));
    assert_eq!(unit.as_bool(), None);
    assert_eq!(number.as_number(), Some(&Number::I8(7)));
    assert_eq!(unit.as_number(), None);
    assert_eq!(text.as_str(), Some("key"));
    assert_eq!(unit.as_str(), None);
    assert_eq!(bytes.as_bytes(), Some(&[1, 2][..]));
    assert_eq!(unit.as_bytes(), None);
    assert_eq!(values.as_sequence().unwrap().len(), 1);
    assert!(unit.as_sequence().is_none());
    assert_eq!(entries.as_map().unwrap().len(), 3);
    assert!(unit.as_map().is_none());
    assert_eq!(entries.get("key").unwrap().as_number(), Some(&Number::U8(1)));
    assert!(entries.get("missing").is_none());
    assert!(unit.get("key").is_none());
    assert_eq!(
        entries.get_all("key").map(|value| *value.as_number().unwrap()).collect::<Vec<_>>(),
        [Number::U8(1), Number::U8(2)]
    );
    assert_eq!(unit.get_all("key").count(), 0);
}

#[test]
fn serialize_all_value_and_number_variants() {
    let arena = Arena::new();
    let numbers = [
        Number::I8(-1),
        Number::I16(-2),
        Number::I32(-3),
        Number::I64(-4),
        Number::I128(-5),
        Number::U8(1),
        Number::U16(2),
        Number::U32(3),
        Number::U64(4),
        Number::U128(5),
        Number::F32(1.5),
        Number::F64(2.5),
    ];
    for number in numbers {
        assert!(!serde_json::to_string(&number).unwrap().is_empty());
        assert!(
            !serde_json::to_string(&Value::<allocator_api2::alloc::Global>::Number(number))
                .unwrap()
                .is_empty()
        );
    }

    let values = [
        Value::Unit,
        Value::None,
        Value::Some(boxed(&arena, Value::Bool(true))),
        Value::Bool(false),
        Value::Char('x'),
        Value::String(string(&arena, "text")),
        Value::Bytes(arena.alloc_slice_copy_box([1_u8, 2])),
        Value::Newtype(boxed(&arena, Value::Number(Number::I32(3)))),
        Value::Sequence(array(&arena, vec![Value::Bool(true), Value::Unit])),
        Value::Map(map(
            &arena,
            vec![Entry {
                key: Value::String(string(&arena, "field")),
                value: Value::Bool(true),
            }],
        )),
        Value::Enum {
            variant: string(&arena, "Unit"),
            value: EnumValue::Unit,
        },
        Value::Enum {
            variant: string(&arena, "New"),
            value: EnumValue::Newtype(boxed(&arena, Value::Number(Number::I32(1)))),
        },
        Value::Enum {
            variant: string(&arena, "Tuple"),
            value: EnumValue::Tuple(array(&arena, vec![Value::Bool(true), Value::Unit])),
        },
        Value::Enum {
            variant: string(&arena, "Struct"),
            value: EnumValue::Struct(map(
                &arena,
                vec![Entry {
                    key: Value::String(string(&arena, "x")),
                    value: Value::Number(Number::I32(1)),
                }],
            )),
        },
    ];
    for value in values {
        assert!(!serde_json::to_string(&value).unwrap().is_empty());
    }
}

#[test]
fn value_visitor_covers_owned_borrowed_and_compound_callbacks() {
    let arena = Arena::new();
    type Error = de::value::Error;

    assert_eq!(ValueVisitor { arena: &arena }.visit_str::<Error>("a").unwrap().as_str(), Some("a"));
    assert_eq!(
        ValueVisitor { arena: &arena }.visit_borrowed_str::<Error>("b").unwrap().as_str(),
        Some("b")
    );
    assert_eq!(
        ValueVisitor { arena: &arena }
            .visit_string::<Error>("c".to_string())
            .unwrap()
            .as_str(),
        Some("c")
    );
    assert_eq!(
        ValueVisitor { arena: &arena }.visit_bytes::<Error>(&[1]).unwrap().as_bytes(),
        Some(&[1][..])
    );
    assert_eq!(
        ValueVisitor { arena: &arena }
            .visit_borrowed_bytes::<Error>(&[2])
            .unwrap()
            .as_bytes(),
        Some(&[2][..])
    );
    assert_eq!(
        ValueVisitor { arena: &arena }.visit_byte_buf::<Error>(vec![3]).unwrap().as_bytes(),
        Some(&[3][..])
    );

    struct Expecting<T>(T);
    impl<T> core_fmt::Display for Expecting<T>
    where
        for<'de> T: Visitor<'de>,
    {
        fn fmt(&self, formatter: &mut core_fmt::Formatter<'_>) -> core_fmt::Result {
            self.0.expecting(formatter)
        }
    }
    assert_eq!(format!("{}", Expecting(ValueVisitor { arena: &arena })), "any Serde value");

    struct OpaqueEnum;
    impl<'de> EnumAccess<'de> for OpaqueEnum {
        type Error = Error;
        type Variant = Self;
        fn variant_seed<V: DeserializeSeed<'de>>(self, _seed: V) -> Result<(V::Value, Self), Error> {
            Err(de::Error::custom("unused"))
        }
    }
    impl<'de> VariantAccess<'de> for OpaqueEnum {
        type Error = Error;
        fn unit_variant(self) -> Result<(), Error> {
            Ok(())
        }
        fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, _seed: T) -> Result<T::Value, Error> {
            Err(de::Error::custom("unused"))
        }
        fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _visitor: V) -> Result<V::Value, Error> {
            Err(de::Error::custom("unused"))
        }
        fn struct_variant<V: Visitor<'de>>(self, _fields: &'static [&'static str], _visitor: V) -> Result<V::Value, Error> {
            Err(de::Error::custom("unused"))
        }
    }
    assert!(ValueVisitor { arena: &arena }.visit_enum(OpaqueEnum).is_err());
}

#[test]
fn deserialize_in_visits_every_scalar_callback_and_compound_shape() {
    let arena = Arena::new();
    type Error = de::value::Error;
    assert!(matches!(ValueVisitor { arena: &arena }.visit_unit::<Error>().unwrap(), Value::Unit));
    assert!(matches!(
        ValueVisitor { arena: &arena }.visit_bool::<Error>(false).unwrap(),
        Value::Bool(false)
    ));
    macro_rules! visit_number {
        ($method:ident, $value:expr, $variant:ident) => {
            assert!(matches!(
                ValueVisitor { arena: &arena }.$method::<Error>($value).unwrap(),
                Value::Number(Number::$variant(value)) if value == $value
            ));
        };
    }
    visit_number!(visit_i8, -1_i8, I8);
    visit_number!(visit_i16, -2_i16, I16);
    visit_number!(visit_i32, -3_i32, I32);
    visit_number!(visit_i64, -4_i64, I64);
    visit_number!(visit_i128, -5_i128, I128);
    visit_number!(visit_u8, 1_u8, U8);
    visit_number!(visit_u16, 2_u16, U16);
    visit_number!(visit_u32, 3_u32, U32);
    visit_number!(visit_u64, 4_u64, U64);
    visit_number!(visit_u128, 5_u128, U128);
    visit_number!(visit_f32, 1.5_f32, F32);
    visit_number!(visit_f64, 2.5_f64, F64);
    assert!(matches!(
        ValueVisitor { arena: &arena }.visit_char::<Error>('x').unwrap(),
        Value::Char('x')
    ));

    let mut json = serde_json::Deserializer::from_str(r#"{"array":[null,true,-1,2,1.5,"s"],"object":{"nested":false}}"#);
    let value = Value::deserialize_in(&arena, &mut json).unwrap();
    assert_eq!(value.as_map().unwrap().len(), 2);

    let deserializer: de::value::I32Deserializer<de::value::Error> = 1_i32.into_deserializer();
    let some = ValueVisitor { arena: &arena }.visit_some(deserializer).unwrap();
    assert!(matches!(some, Value::Some(_)));
    let deserializer: de::value::BoolDeserializer<de::value::Error> = false.into_deserializer();
    let newtype = ValueVisitor { arena: &arena }.visit_newtype_struct(deserializer).unwrap();
    assert!(matches!(newtype, Value::Newtype(_)));
    assert!(matches!(
        ValueVisitor { arena: &arena }.visit_none::<de::value::Error>().unwrap(),
        Value::None
    ));
}

macro_rules! try_integer_targets {
    ($value:expr) => {{
        let value: Value = $value;
        let _ = i8::deserialize(&value);
        let _ = i16::deserialize(&value);
        let _ = i32::deserialize(&value);
        let _ = i64::deserialize(&value);
        let _ = i128::deserialize(&value);
        let _ = u8::deserialize(&value);
        let _ = u16::deserialize(&value);
        let _ = u32::deserialize(&value);
        let _ = u64::deserialize(&value);
        let _ = u128::deserialize(&value);
    }};
}

#[test]
fn replay_integer_conversion_matrix_and_errors() {
    try_integer_targets!(Value::Number(Number::I8(-1)));
    try_integer_targets!(Value::Number(Number::I16(-2)));
    try_integer_targets!(Value::Number(Number::I32(-3)));
    try_integer_targets!(Value::Number(Number::I64(-4)));
    try_integer_targets!(Value::Number(Number::I128(i128::MIN)));
    try_integer_targets!(Value::Number(Number::U8(1)));
    try_integer_targets!(Value::Number(Number::U16(2)));
    try_integer_targets!(Value::Number(Number::U32(3)));
    try_integer_targets!(Value::Number(Number::U64(4)));
    try_integer_targets!(Value::Number(Number::U128(u128::MAX)));
    try_integer_targets!(Value::Number(Number::F32(1.0)));
    try_integer_targets!(Value::Number(Number::F64(1.0)));
    try_integer_targets!(Value::Unit);

    let error = i8::deserialize(&Value::<allocator_api2::alloc::Global>::Number(Number::F32(1.5)))
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected i8"));
    let error = u8::deserialize(&Value::<allocator_api2::alloc::Global>::Number(Number::I8(-1)))
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected u8"));
    let error = i8::deserialize(&Value::<allocator_api2::alloc::Global>::Number(Number::U16(256)))
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected i8"));
}

#[test]
fn replay_typed_numeric_methods_preserve_every_number_kind() {
    struct NumericCallback;

    macro_rules! callbacks {
        ($($method:ident($type:ty) => $name:literal),+ $(,)?) => {
            $(
                fn $method<E: de::Error>(self, _value: $type) -> Result<Self::Value, E> {
                    Ok($name)
                }
            )+
        };
    }

    impl Visitor<'_> for NumericCallback {
        type Value = &'static str;

        fn expecting(&self, formatter: &mut core_fmt::Formatter<'_>) -> core_fmt::Result {
            formatter.write_str("a number")
        }

        callbacks! {
            visit_i8(i8) => "i8", visit_i16(i16) => "i16", visit_i32(i32) => "i32",
            visit_i64(i64) => "i64", visit_i128(i128) => "i128",
            visit_u8(u8) => "u8", visit_u16(u16) => "u16", visit_u32(u32) => "u32",
            visit_u64(u64) => "u64", visit_u128(u128) => "u128",
            visit_f32(f32) => "f32", visit_f64(f64) => "f64",
        }
    }

    macro_rules! assert_float_callback {
        ($method:ident, $variant:ident($input:expr), $expected:literal) => {
            assert_eq!(
                (&Value::<allocator_api2::alloc::Global>::Number(Number::$variant($input)))
                    .$method(NumericCallback)
                    .unwrap(),
                $expected
            );
        };
    }

    assert_float_callback!(deserialize_f32, F32(1.5), "f32");
    assert_float_callback!(deserialize_f32, F64(2.5), "f64");
    assert_float_callback!(deserialize_f32, I8(-8), "i8");
    assert_float_callback!(deserialize_f32, I16(-16), "i16");
    assert_float_callback!(deserialize_f32, I32(-32), "i32");
    assert_float_callback!(deserialize_f32, I64(-64), "i64");
    assert_float_callback!(deserialize_f32, I128(-128), "i128");
    assert_float_callback!(deserialize_f32, U8(8), "u8");
    assert_float_callback!(deserialize_f32, U16(16), "u16");
    assert_float_callback!(deserialize_f32, U32(32), "u32");
    assert_float_callback!(deserialize_f32, U64(64), "u64");
    assert_float_callback!(deserialize_f32, U128(128), "u128");

    assert_float_callback!(deserialize_i8, U8(8), "u8");
    assert_float_callback!(deserialize_u8, I8(8), "i8");

    assert_float_callback!(deserialize_f64, F32(1.5), "f32");
    assert_float_callback!(deserialize_f64, F64(2.5), "f64");
    assert_float_callback!(deserialize_f64, I8(-8), "i8");
    assert_float_callback!(deserialize_f64, I16(-16), "i16");
    assert_float_callback!(deserialize_f64, I32(-32), "i32");
    assert_float_callback!(deserialize_f64, I64(-64), "i64");
    assert_float_callback!(deserialize_f64, I128(-128), "i128");
    assert_float_callback!(deserialize_f64, U8(8), "u8");
    assert_float_callback!(deserialize_f64, U16(16), "u16");
    assert_float_callback!(deserialize_f64, U32(32), "u32");
    assert_float_callback!(deserialize_f64, U64(64), "u64");
    assert_float_callback!(deserialize_f64, U128(128), "u128");
}

#[test]
fn replay_any_and_typed_deserializer_paths() {
    let arena = Arena::new();
    let scalars = vec![
        Value::Unit,
        Value::None,
        Value::Some(boxed(&arena, Value::Bool(true))),
        Value::Bool(true),
        Value::Number(Number::I8(-1)),
        Value::Number(Number::I16(-2)),
        Value::Number(Number::I32(-3)),
        Value::Number(Number::I64(-4)),
        Value::Number(Number::I128(-5)),
        Value::Number(Number::U8(1)),
        Value::Number(Number::U16(2)),
        Value::Number(Number::U32(3)),
        Value::Number(Number::U64(4)),
        Value::Number(Number::U128(5)),
        Value::Number(Number::F32(1.5)),
        Value::Number(Number::F64(2.5)),
        Value::Char('x'),
        Value::String(string(&arena, "s")),
        Value::Bytes(arena.alloc_slice_copy_box([1_u8])),
        Value::Newtype(boxed(&arena, Value::Bool(false))),
        Value::Sequence(array(&arena, vec![Value::Bool(true)])),
        Value::Map(map(
            &arena,
            vec![Entry {
                key: Value::String(string(&arena, "x")),
                value: Value::Bool(true),
            }],
        )),
    ];
    for value in &scalars {
        let replay_arena = Arena::new();
        let _ = Value::deserialize_in(&replay_arena, value);
    }

    assert!(bool::deserialize(&Value::<allocator_api2::alloc::Global>::Bool(true)).unwrap());
    assert!(bool::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());
    assert_eq!(char::deserialize(&Value::<allocator_api2::alloc::Global>::Char('z')).unwrap(), 'z');
    assert_eq!(
        char::deserialize(&Value::<allocator_api2::alloc::Global>::String(string(&arena, "q"))).unwrap(),
        'q'
    );
    assert!(char::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());
    assert_eq!(
        String::deserialize(&Value::<allocator_api2::alloc::Global>::String(string(&arena, "text"))).unwrap(),
        "text"
    );
    assert!(String::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());

    for number in scalars.iter().filter_map(Value::as_number) {
        let value: Value = Value::Number(*number);
        let _ = f32::deserialize(&value);
        let _ = f64::deserialize(&value);
    }
    assert!(f32::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());
    assert!(f64::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());

    assert_eq!(
        Option::<bool>::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).unwrap(),
        None
    );
    assert_eq!(
        Option::<bool>::deserialize(&Value::<allocator_api2::alloc::Global>::None).unwrap(),
        None
    );
    assert_eq!(
        Option::<bool>::deserialize(&Value::<allocator_api2::alloc::Global>::Some(boxed(&arena, Value::Bool(true)))).unwrap(),
        Some(true)
    );
    assert_eq!(
        Option::<bool>::deserialize(&Value::<allocator_api2::alloc::Global>::Bool(false)).unwrap(),
        Some(false)
    );
    assert!(<()>::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_ok());
    assert!(<()>::deserialize(&Value::<allocator_api2::alloc::Global>::Bool(true)).is_err());

    #[derive(Debug, Deserialize, PartialEq)]
    struct UnitStruct;
    #[derive(Debug, Deserialize, PartialEq)]
    struct Newtype(bool);
    #[derive(Debug, Deserialize, PartialEq)]
    struct Pair(i32, i32);
    assert_eq!(
        UnitStruct::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).unwrap(),
        UnitStruct
    );
    assert_eq!(
        Newtype::deserialize(&Value::<allocator_api2::alloc::Global>::Newtype(boxed(&arena, Value::Bool(true)))).unwrap(),
        Newtype(true)
    );
    assert_eq!(
        Newtype::deserialize(&Value::<allocator_api2::alloc::Global>::Bool(false)).unwrap(),
        Newtype(false)
    );

    let sequence = Value::Sequence(array(&arena, vec![Value::Number(Number::I32(1)), Value::Number(Number::I32(2))]));
    assert_eq!(Vec::<i32>::deserialize(&sequence).unwrap(), [1, 2]);
    assert_eq!(<(i32, i32)>::deserialize(&sequence).unwrap(), (1, 2));
    assert_eq!(Pair::deserialize(&sequence).unwrap(), Pair(1, 2));
    assert_eq!(
        <(i32,)>::deserialize(&sequence).unwrap_err().to_string(),
        "invalid type sequence of wrong length, expected a tuple"
    );
    assert!(Vec::<i32>::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());
    assert!(<(i32,)>::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());

    let map = Value::Map(map(
        &arena,
        vec![Entry {
            key: Value::String(string(&arena, "x")),
            value: Value::Number(Number::I32(4)),
        }],
    ));
    assert_eq!(BTreeMap::<String, i32>::deserialize(&map).unwrap()["x"], 4);
    assert!(BTreeMap::<String, i32>::deserialize(&Value::<allocator_api2::alloc::Global>::Unit).is_err());

    #[derive(Deserialize)]
    struct Record {
        x: i32,
    }
    assert_eq!(Record::deserialize(&map).unwrap().x, 4);
    let _: de::IgnoredAny = Deserialize::deserialize(&Value::<allocator_api2::alloc::Global>::Bool(true)).unwrap();
}

struct BytesVisitor;

impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Vec<u8>;
    fn expecting(&self, formatter: &mut core_fmt::Formatter<'_>) -> core_fmt::Result {
        formatter.write_str("bytes")
    }
    fn visit_borrowed_bytes<E: de::Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
        Ok(value.to_vec())
    }
}

#[test]
fn replay_bytes_identifiers_aliases_and_replay_accessors() {
    let arena = Arena::new();
    let bytes = Value::Bytes(arena.alloc_slice_copy_box([1_u8, 2]));
    assert_eq!((&bytes).deserialize_bytes(BytesVisitor).unwrap(), [1, 2]);
    assert_eq!((&bytes).deserialize_byte_buf(BytesVisitor).unwrap(), [1, 2]);
    assert!(
        (&Value::<allocator_api2::alloc::Global>::Unit)
            .deserialize_bytes(BytesVisitor)
            .is_err()
    );

    let text = Value::String(string(&arena, "id"));
    assert_eq!(String::deserialize(&text).unwrap(), "id");
    assert!(
        (&Value::<allocator_api2::alloc::Global>::Unit)
            .deserialize_identifier(BytesVisitor)
            .is_err()
    );

    let sequence = Value::Sequence(array(&arena, vec![Value::Bool(true)]));
    assert_eq!(Vec::<bool>::deserialize(&sequence).unwrap(), [true]);
    let mut seq = SeqReplay::new(sequence.as_sequence().unwrap());
    assert_eq!(seq.size_hint(), Some(1));
    assert_eq!(seq.next_element::<bool>().unwrap(), Some(true));
    assert_eq!(seq.size_hint(), Some(0));
    assert_eq!(seq.next_element::<bool>().unwrap(), None);

    let entries = [Entry {
        key: Value::String(string(&arena, "key")),
        value: Value::Bool(true),
    }];
    let mut map = MapReplay::new(&entries);
    assert_eq!(map.size_hint(), Some(1));
    assert!(map.next_value::<bool>().is_err());
    assert_eq!(map.next_key::<String>().unwrap().as_deref(), Some("key"));
    assert!(map.next_value::<bool>().unwrap());
    assert_eq!(map.next_key::<String>().unwrap(), None);
    assert_eq!(map.size_hint(), Some(0));
}

#[derive(Debug, Deserialize, PartialEq)]
enum TestEnum {
    Unit,
    New(i32),
    Tuple(i32, bool),
    Struct { x: i32 },
}

fn explicit_enum(arena: &Arena, variant: &str, value: EnumValue) -> Value {
    Value::Enum {
        variant: string(arena, variant),
        value,
    }
}

fn external_enum(arena: &Arena, variant: &str, value: Value) -> Value {
    Value::Map(map(
        arena,
        vec![Entry {
            key: Value::String(string(arena, variant)),
            value,
        }],
    ))
}

#[test]
fn replay_all_enum_representations_and_payload_errors() {
    let arena = Arena::new();
    let unit_string = Value::String(string(&arena, "Unit"));
    assert_eq!(TestEnum::deserialize(&unit_string).unwrap(), TestEnum::Unit);
    assert_eq!(
        TestEnum::deserialize(&explicit_enum(&arena, "Unit", EnumValue::Unit)).unwrap(),
        TestEnum::Unit
    );
    let replay_arena = Arena::new();
    assert!(Value::deserialize_in(&replay_arena, &explicit_enum(&arena, "Unit", EnumValue::Unit)).is_err());
    assert_eq!(
        TestEnum::deserialize(&explicit_enum(
            &arena,
            "New",
            EnumValue::Newtype(boxed(&arena, Value::Number(Number::I32(1))))
        ))
        .unwrap(),
        TestEnum::New(1)
    );
    assert_eq!(
        TestEnum::deserialize(&explicit_enum(
            &arena,
            "Tuple",
            EnumValue::Tuple(array(&arena, vec![Value::Number(Number::I32(2)), Value::Bool(true)]))
        ))
        .unwrap(),
        TestEnum::Tuple(2, true)
    );
    assert_eq!(
        TestEnum::deserialize(&explicit_enum(
            &arena,
            "Struct",
            EnumValue::Struct(map(
                &arena,
                vec![Entry {
                    key: Value::String(string(&arena, "x")),
                    value: Value::Number(Number::I32(3))
                }]
            ))
        ))
        .unwrap(),
        TestEnum::Struct { x: 3 }
    );

    assert_eq!(
        TestEnum::deserialize(&external_enum(&arena, "Unit", Value::Unit)).unwrap(),
        TestEnum::Unit
    );
    assert_eq!(
        TestEnum::deserialize(&external_enum(&arena, "New", Value::Number(Number::I32(4)))).unwrap(),
        TestEnum::New(4)
    );
    assert_eq!(
        TestEnum::deserialize(&external_enum(
            &arena,
            "Tuple",
            Value::Sequence(array(&arena, vec![Value::Number(Number::I32(5)), Value::Bool(false)]))
        ))
        .unwrap(),
        TestEnum::Tuple(5, false)
    );
    assert_eq!(
        TestEnum::deserialize(&external_enum(
            &arena,
            "Struct",
            Value::Map(map(
                &arena,
                vec![Entry {
                    key: Value::String(string(&arena, "x")),
                    value: Value::Number(Number::I32(6))
                }]
            ))
        ))
        .unwrap(),
        TestEnum::Struct { x: 6 }
    );

    assert!(TestEnum::deserialize(&explicit_enum(&arena, "Unit", EnumValue::Newtype(boxed(&arena, Value::Unit)))).is_err());
    assert!(TestEnum::deserialize(&explicit_enum(&arena, "New", EnumValue::Unit)).is_err());
    assert!(TestEnum::deserialize(&explicit_enum(&arena, "Tuple", EnumValue::Unit)).is_err());
    assert!(TestEnum::deserialize(&explicit_enum(&arena, "Struct", EnumValue::Unit)).is_err());
    assert_eq!(
        TestEnum::deserialize(&explicit_enum(
            &arena,
            "Tuple",
            EnumValue::Tuple(array(&arena, vec![Value::Number(Number::I32(1))]))
        ))
        .unwrap_err()
        .to_string(),
        "invalid type invalid enum payload, expected a tuple variant"
    );
    assert!(TestEnum::deserialize(&external_enum(&arena, "Unit", Value::Bool(true))).is_err());
    assert!(TestEnum::deserialize(&external_enum(&arena, "Tuple", Value::Unit)).is_err());
    assert!(TestEnum::deserialize(&external_enum(&arena, "Struct", Value::Unit)).is_err());

    let non_string_tag = Value::Map(map(
        &arena,
        vec![Entry {
            key: Value::Bool(true),
            value: Value::Unit,
        }],
    ));
    assert!(TestEnum::deserialize(&non_string_tag).is_err());
    assert!(TestEnum::deserialize(&Value::<allocator_api2::alloc::Global>::Map(map(&arena, vec![]))).is_err());
    assert!(TestEnum::deserialize(&Value::<allocator_api2::alloc::Global>::Bool(true)).is_err());
}
