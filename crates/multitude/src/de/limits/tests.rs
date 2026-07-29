// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::too_many_lines, reason = "coverage test exercises the complete visitor dispatch")]

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use serde::de::IntoDeserializer;
use serde::de::value::{Error, MapDeserializer, SeqDeserializer};

use super::*;

fn make_state(limits: DeserializationLimits) -> LimitState {
    LimitState {
        limits,
        depth: Cell::new(0),
        limit_exceeded: Cell::new(None),
    }
}

fn limits() -> DeserializationLimits {
    DeserializationLimits::unlimited()
        .with_max_depth(8)
        .with_max_sequence_len(8)
        .with_max_map_len(8)
        .with_max_string_len(8)
        .with_max_bytes_len(8)
}

struct Accept;

impl<'de> Visitor<'de> for Accept {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("accepted value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i8<E>(self, _: i8) -> Result<(), E> {
        Ok(())
    }
    fn visit_i16<E>(self, _: i16) -> Result<(), E> {
        Ok(())
    }
    fn visit_i32<E>(self, _: i32) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_i128<E>(self, _: i128) -> Result<(), E> {
        Ok(())
    }
    fn visit_u8<E>(self, _: u8) -> Result<(), E> {
        Ok(())
    }
    fn visit_u16<E>(self, _: u16) -> Result<(), E> {
        Ok(())
    }
    fn visit_u32<E>(self, _: u32) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u128<E>(self, _: u128) -> Result<(), E> {
        Ok(())
    }
    fn visit_f32<E>(self, _: f32) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_char<E>(self, _: char) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_borrowed_str<E>(self, _: &'de str) -> Result<(), E> {
        Ok(())
    }
    fn visit_string<E>(self, _: String) -> Result<(), E> {
        Ok(())
    }
    fn visit_bytes<E>(self, _: &[u8]) -> Result<(), E> {
        Ok(())
    }
    fn visit_borrowed_bytes<E>(self, _: &'de [u8]) -> Result<(), E> {
        Ok(())
    }
    fn visit_byte_buf<E>(self, _: Vec<u8>) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(Self)
    }
    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(Self)
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
        while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
        Ok(())
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while map.next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?.is_some() {}
        Ok(())
    }
    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<(), A::Error> {
        let (_, variant) = data.variant::<serde::de::IgnoredAny>()?;
        variant.unit_variant()
    }
}

struct UnitDeserializer;

impl<'de> Deserializer<'de> for UnitDeserializer {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

fn limited(state: &LimitState) -> LimitedDeserializer<'_, UnitDeserializer> {
    LimitedDeserializer {
        inner: UnitDeserializer,
        state,
    }
}

#[test]
fn delegates_every_deserializer_method() {
    let state = make_state(limits());
    limited(&state).deserialize_any(Accept).unwrap();
    limited(&state).deserialize_bool(Accept).unwrap();
    limited(&state).deserialize_i8(Accept).unwrap();
    limited(&state).deserialize_i16(Accept).unwrap();
    limited(&state).deserialize_i32(Accept).unwrap();
    limited(&state).deserialize_i64(Accept).unwrap();
    limited(&state).deserialize_i128(Accept).unwrap();
    limited(&state).deserialize_u8(Accept).unwrap();
    limited(&state).deserialize_u16(Accept).unwrap();
    limited(&state).deserialize_u32(Accept).unwrap();
    limited(&state).deserialize_u64(Accept).unwrap();
    limited(&state).deserialize_u128(Accept).unwrap();
    limited(&state).deserialize_f32(Accept).unwrap();
    limited(&state).deserialize_f64(Accept).unwrap();
    limited(&state).deserialize_char(Accept).unwrap();
    limited(&state).deserialize_str(Accept).unwrap();
    limited(&state).deserialize_string(Accept).unwrap();
    limited(&state).deserialize_bytes(Accept).unwrap();
    limited(&state).deserialize_byte_buf(Accept).unwrap();
    limited(&state).deserialize_option(Accept).unwrap();
    limited(&state).deserialize_unit(Accept).unwrap();
    limited(&state).deserialize_unit_struct("U", Accept).unwrap();
    limited(&state).deserialize_newtype_struct("N", Accept).unwrap();
    limited(&state).deserialize_seq(Accept).unwrap();
    limited(&state).deserialize_tuple(0, Accept).unwrap();
    limited(&state).deserialize_tuple_struct("T", 0, Accept).unwrap();
    limited(&state).deserialize_map(Accept).unwrap();
    limited(&state).deserialize_struct("S", &[], Accept).unwrap();
    limited(&state).deserialize_enum("E", &[], Accept).unwrap();
    limited(&state).deserialize_identifier(Accept).unwrap();
    limited(&state).deserialize_ignored_any(Accept).unwrap();
}

struct Expected<'a>(&'a LimitState);

impl fmt::Display for Expected<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        LimitedVisitor {
            inner: Accept,
            state: self.0,
        }
        .expecting(formatter)
    }
}

#[test]
fn forwards_scalar_string_bytes_option_and_unit_callbacks() {
    let state = make_state(limits());
    assert_eq!(Expected(&state).to_string(), "accepted value");

    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_bool::<Error>(true)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_i8::<Error>(-1)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_i16::<Error>(-2)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_i32::<Error>(-3)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_i64::<Error>(-4)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_i128::<Error>(-5)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_u8::<Error>(1)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_u16::<Error>(2)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_u32::<Error>(3)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_u64::<Error>(4)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_u128::<Error>(5)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_f32::<Error>(1.5)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_f64::<Error>(2.5)
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_char::<Error>('x')
    .unwrap();

    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_str::<Error>("12345678")
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_borrowed_str::<Error>("borrowed")
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_string::<Error>("owned".to_string())
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_bytes::<Error>(b"12345678")
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_borrowed_bytes::<Error>(b"borrowed")
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_byte_buf::<Error>(vec![1, 2])
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_none::<Error>()
    .unwrap();
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_unit::<Error>()
    .unwrap();

    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_str::<Error>("123456789")
    .unwrap_err();
    assert!(err.to_string().contains("string length"));
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_borrowed_str::<Error>("123456789")
    .unwrap_err();
    assert!(err.to_string().contains("string length"));
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_string::<Error>("123456789".to_string())
    .unwrap_err();
    assert!(err.to_string().contains("string length"));
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_bytes::<Error>(b"123456789")
    .unwrap_err();
    assert!(err.to_string().contains("byte string length"));
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_borrowed_bytes::<Error>(b"123456789")
    .unwrap_err();
    assert!(err.to_string().contains("byte string length"));
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_byte_buf::<Error>(vec![0; 9])
    .unwrap_err();
    assert!(err.to_string().contains("byte string length"));
}

struct FailSeed;

impl<'de> DeserializeSeed<'de> for FailSeed {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
        Err(<D::Error as serde::de::Error>::custom("seed failure"))
    }
}

struct FailNested;

impl<'de> Visitor<'de> for FailNested {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("nested value")
    }

    fn visit_some<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
        Err(<D::Error as serde::de::Error>::custom("some failure"))
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
        Err(<D::Error as serde::de::Error>::custom("newtype failure"))
    }
}

#[test]
fn depth_enter_leave_success_limit_and_nested_errors() {
    let mut configured = limits();
    configured.max_depth = 1;
    let state = make_state(configured);

    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_some(serde::de::value::U8Deserializer::<Error>::new(1))
    .unwrap();
    assert_eq!(state.depth.get(), 0);
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_newtype_struct(serde::de::value::U8Deserializer::<Error>::new(1))
    .unwrap();
    assert_eq!(state.depth.get(), 0);

    state.depth.set(1);
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_some(serde::de::value::U8Deserializer::<Error>::new(1))
    .unwrap_err();
    assert!(err.to_string().contains("nesting depth"));
    assert_eq!(state.depth.get(), 1);
    let err = LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_newtype_struct(serde::de::value::U8Deserializer::<Error>::new(1))
    .unwrap_err();
    assert!(err.to_string().contains("nesting depth"));
    assert_eq!(state.depth.get(), 1);

    state.depth.set(0);
    let err = LimitedSeed {
        inner: FailSeed,
        state: &state,
    }
    .deserialize(UnitDeserializer)
    .unwrap_err();
    assert!(err.to_string().contains("seed failure"));
    assert_eq!(state.depth.get(), 0);

    LimitedVisitor {
        inner: FailNested,
        state: &state,
    }
    .visit_some(UnitDeserializer)
    .unwrap_err();
    assert_eq!(state.depth.get(), 0);
    LimitedVisitor {
        inner: FailNested,
        state: &state,
    }
    .visit_newtype_struct(UnitDeserializer)
    .unwrap_err();
    assert_eq!(state.depth.get(), 0);

    let unrestricted = make_state(DeserializationLimits::default());
    unrestricted.depth.set(usize::MAX);
    unrestricted.enter::<Error>().unwrap();
    assert_eq!(unrestricted.depth.get(), usize::MAX);
    unrestricted.leave();
    assert_eq!(unrestricted.depth.get(), usize::MAX);
}

#[test]
fn depth_is_restored_when_a_seed_panics() {
    struct PanicSeed;

    impl<'de> DeserializeSeed<'de> for PanicSeed {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
            panic!("seed panic")
        }
    }

    let state = make_state(limits());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = LimitedSeed {
            inner: PanicSeed,
            state: &state,
        }
        .deserialize(UnitDeserializer);
    }));

    assert!(result.is_err());
    assert_eq!(state.depth.get(), 0);
}

#[test]
fn depth_is_restored_when_nested_visitors_panic() {
    struct PanicNested;

    impl<'de> Visitor<'de> for PanicNested {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a panic")
        }

        fn visit_some<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
            panic!("option visitor panic")
        }

        fn visit_newtype_struct<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
            panic!("newtype visitor panic")
        }
    }

    let state = make_state(limits());
    let option = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = LimitedVisitor {
            inner: PanicNested,
            state: &state,
        }
        .visit_some(UnitDeserializer);
    }));
    assert!(option.is_err());
    assert_eq!(state.depth.get(), 0);

    let newtype = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = LimitedVisitor {
            inner: PanicNested,
            state: &state,
        }
        .visit_newtype_struct(UnitDeserializer);
    }));
    assert!(newtype.is_err());
    assert_eq!(state.depth.get(), 0);
}

#[test]
fn top_level_seed_and_default_limits_work() {
    assert_eq!(DeserializationLimits::default(), DeserializationLimits::unlimited());
    let value = deserialize_seed_with_limits(
        serde::de::value::U8Deserializer::<Error>::new(7),
        core::marker::PhantomData::<u8>,
        DeserializationLimits::default(),
    )
    .unwrap();
    assert_eq!(value, 7);
}

#[test]
fn sequence_callbacks_hints_end_overflow_and_depth_error() {
    let mut configured = limits();
    configured.max_sequence_len = 2;
    let state = make_state(configured);
    let inner = SeqDeserializer::<_, Error>::new(vec![1_u8, 2, 3].into_iter());
    let mut seq = LimitedSeqAccess {
        inner,
        state: &state,
        count: 0,
    };
    assert_eq!(seq.size_hint(), Some(2));
    assert_eq!(seq.next_element::<u8>().unwrap(), Some(1));
    assert_eq!(seq.size_hint(), Some(1));
    assert_eq!(seq.next_element::<u8>().unwrap(), Some(2));
    assert_eq!(seq.size_hint(), Some(0));
    let err = seq.next_element_seed(FailSeed).unwrap_err();
    assert!(err.to_string().contains("sequence length"));

    let inner = SeqDeserializer::<_, Error>::new(vec![1_u8, 2].into_iter());
    let mut exact = LimitedSeqAccess {
        inner,
        state: &state,
        count: 0,
    };
    assert_eq!(exact.next_element::<u8>().unwrap(), Some(1));
    assert_eq!(exact.next_element::<u8>().unwrap(), Some(2));
    assert_eq!(exact.next_element::<u8>().unwrap(), None);

    let inner = SeqDeserializer::<_, Error>::new(vec![1_u8].into_iter());
    let mut overflow = LimitedSeqAccess {
        inner,
        state: &state,
        count: usize::MAX,
    };
    assert!(overflow.next_element::<u8>().unwrap_err().to_string().contains("sequence length"));

    let unlimited = make_state(DeserializationLimits::unlimited());
    let inner = SeqDeserializer::<_, Error>::new(vec![1_u8].into_iter());
    let mut uncounted = LimitedSeqAccess {
        inner,
        state: &unlimited,
        count: usize::MAX,
    };
    assert_eq!(uncounted.next_element::<u8>().unwrap(), Some(1));
    assert_eq!(uncounted.count, usize::MAX);

    let inner = SeqDeserializer::<_, Error>::new(Vec::<u8>::new().into_iter());
    let mut empty = LimitedSeqAccess {
        inner,
        state: &state,
        count: 0,
    };
    assert_eq!(empty.next_element::<u8>().unwrap(), None);

    let mut no_hint = LimitedSeqAccess {
        inner: NoHintSeq,
        state: &state,
        count: 0,
    };
    assert_eq!(no_hint.size_hint(), None);
    assert_eq!(no_hint.next_element::<u8>().unwrap(), None);

    let mut shallow = limits();
    shallow.max_depth = 0;
    let shallow = make_state(shallow);
    let inner = SeqDeserializer::<_, Error>::new(vec![1_u8].into_iter());
    let mut seq = LimitedSeqAccess {
        inner,
        state: &shallow,
        count: 0,
    };
    assert!(seq.next_element::<u8>().unwrap_err().to_string().contains("nesting depth"));

    let inner = SeqDeserializer::<_, Error>::new(Vec::<u8>::new().into_iter());
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_seq(inner)
    .unwrap();
}

struct NoHintSeq;

impl<'de> SeqAccess<'de> for NoHintSeq {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, _: T) -> Result<Option<T::Value>, Error> {
        Ok(None)
    }
}

#[test]
fn map_callbacks_hints_end_overflow_values_and_depth_error() {
    let mut configured = limits();
    configured.max_map_len = 1;
    let state = make_state(configured);
    let entries = vec![("a", 1_u8), ("b", 2_u8)];
    let inner = MapDeserializer::<_, Error>::new(entries.into_iter());
    let mut map = LimitedMapAccess {
        inner,
        state: &state,
        count: 0,
    };
    assert_eq!(map.size_hint(), Some(1));
    assert_eq!(map.next_key::<String>().unwrap().as_deref(), Some("a"));
    assert_eq!(map.size_hint(), Some(0));
    assert_eq!(map.next_value::<u8>().unwrap(), 1);
    assert!(map.next_key_seed(FailSeed).unwrap_err().to_string().contains("map length"));

    let inner = MapDeserializer::<_, Error>::new(vec![("a", 1_u8)].into_iter());
    let mut exact = LimitedMapAccess {
        inner,
        state: &state,
        count: 0,
    };
    assert_eq!(exact.next_key::<String>().unwrap().as_deref(), Some("a"));
    assert_eq!(exact.next_value::<u8>().unwrap(), 1);
    assert_eq!(exact.next_key::<String>().unwrap(), None);

    let inner = MapDeserializer::<_, Error>::new(vec![("a", 1_u8)].into_iter());
    let mut overflow = LimitedMapAccess {
        inner,
        state: &state,
        count: usize::MAX,
    };
    assert!(overflow.next_key::<String>().unwrap_err().to_string().contains("map length"));

    let unlimited = make_state(DeserializationLimits::unlimited());
    let inner = MapDeserializer::<_, Error>::new(vec![("a", 1_u8)].into_iter());
    let mut uncounted = LimitedMapAccess {
        inner,
        state: &unlimited,
        count: usize::MAX,
    };
    assert_eq!(uncounted.next_key::<String>().unwrap().as_deref(), Some("a"));
    assert_eq!(uncounted.next_value::<u8>().unwrap(), 1);
    assert_eq!(uncounted.count, usize::MAX);

    let inner = MapDeserializer::<_, Error>::new(Vec::<(&str, u8)>::new().into_iter());
    let mut empty = LimitedMapAccess {
        inner,
        state: &state,
        count: 0,
    };
    assert_eq!(empty.next_key::<String>().unwrap(), None);

    let mut no_hint = LimitedMapAccess {
        inner: NoHintMap,
        state: &state,
        count: 0,
    };
    assert_eq!(no_hint.size_hint(), None);
    assert_eq!(no_hint.next_key::<String>().unwrap(), None);

    let mut shallow = limits();
    shallow.max_depth = 0;
    let shallow = make_state(shallow);
    let inner = MapDeserializer::<_, Error>::new(vec![("a", 1_u8)].into_iter());
    let mut map = LimitedMapAccess {
        inner,
        state: &shallow,
        count: 0,
    };
    assert!(map.next_key::<String>().unwrap_err().to_string().contains("nesting depth"));

    let inner = MapDeserializer::<_, Error>::new(Vec::<(&str, u8)>::new().into_iter());
    LimitedVisitor {
        inner: Accept,
        state: &state,
    }
    .visit_map(inner)
    .unwrap();
}

struct NoHintMap;

impl<'de> MapAccess<'de> for NoHintMap {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, _: K) -> Result<Option<K::Value>, Error> {
        Ok(None)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, _: V) -> Result<V::Value, Error> {
        Err(<Error as serde::de::Error>::custom("no value"))
    }
}

#[derive(Clone, Copy)]
enum VariantKind {
    Unit,
    Newtype,
    Tuple,
    Struct,
    FailUnit,
}

struct TestEnum(VariantKind);
struct TestVariant(VariantKind);

impl<'de> EnumAccess<'de> for TestEnum {
    type Error = Error;
    type Variant = TestVariant;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, TestVariant), Error> {
        let name = "variant".into_deserializer();
        Ok((seed.deserialize(name)?, TestVariant(self.0)))
    }
}

impl<'de> VariantAccess<'de> for TestVariant {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        match self.0 {
            VariantKind::FailUnit => Err(<Error as serde::de::Error>::custom("unit failure")),
            _ => Ok(()),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(9_u8.into_deserializer())
    }

    fn tuple_variant<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_seq(SeqDeserializer::new(vec![1_u8].into_iter()))
    }

    fn struct_variant<V: Visitor<'de>>(self, _: &'static [&'static str], visitor: V) -> Result<V::Value, Error> {
        visitor.visit_map(MapDeserializer::new(vec![("x", 1_u8)].into_iter()))
    }
}

struct EnumVisitor(VariantKind);

impl<'de> Visitor<'de> for EnumVisitor {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("enum")
    }
    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<(), A::Error> {
        let (name, variant) = data.variant::<String>()?;
        assert_eq!(name, "variant");
        match self.0 {
            VariantKind::Unit | VariantKind::FailUnit => variant.unit_variant(),
            VariantKind::Newtype => {
                assert_eq!(variant.newtype_variant::<u8>()?, 9);
                Ok(())
            }
            VariantKind::Tuple => variant.tuple_variant(1, Accept),
            VariantKind::Struct => variant.struct_variant(&["x"], Accept),
        }
    }
}

#[test]
fn enum_and_all_variant_access_methods_are_limited() {
    for kind in [VariantKind::Unit, VariantKind::Newtype, VariantKind::Tuple, VariantKind::Struct] {
        let state = make_state(limits());
        LimitedVisitor {
            inner: EnumVisitor(kind),
            state: &state,
        }
        .visit_enum(TestEnum(kind))
        .unwrap();
        assert_eq!(state.depth.get(), 0);
    }

    let state = make_state(limits());
    let err = LimitedVisitor {
        inner: EnumVisitor(VariantKind::FailUnit),
        state: &state,
    }
    .visit_enum(TestEnum(VariantKind::FailUnit))
    .unwrap_err();
    assert!(err.to_string().contains("unit failure"));

    let mut shallow = limits();
    shallow.max_depth = 0;
    let state = make_state(shallow);
    let err = LimitedVisitor {
        inner: EnumVisitor(VariantKind::Unit),
        state: &state,
    }
    .visit_enum(TestEnum(VariantKind::Unit))
    .unwrap_err();
    assert!(err.to_string().contains("nesting depth"));
}
