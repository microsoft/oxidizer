// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Line-level coverage for the public arena serialization adapters.

#![cfg(feature = "serde_json")]
#![allow(clippy::assertions_on_result_states, reason = "tests deliberately exercise allocation errors")]
#![allow(clippy::float_cmp, reason = "JSON round trips preserve these exactly representable test values")]
#![allow(clippy::items_after_statements, reason = "test-local helpers are clearer beside their use")]
#![allow(clippy::unwrap_used, reason = "coverage tests")]

mod common;

use core::fmt::Debug;

use common::{FailingAllocator, SyncFailingAllocator};
use multitude::de::{DeserializationLimits, DeserializeIn, DeserializeInSeed};
use multitude::{Arc, Arena, Box, Cow, Rc};
use serde::de::value::{BorrowedStrDeserializer, Error as ValueError, SeqDeserializer, StringDeserializer, U64Deserializer};
use serde::de::{DeserializeSeed as _, Deserializer, Visitor};

fn unlimited() -> DeserializationLimits {
    DeserializationLimits::unlimited()
}

struct UnitOptionDeserializer;

impl<'de> Deserializer<'de> for UnitOptionDeserializer {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct newtype_struct seq tuple tuple_struct map struct
        enum identifier ignored_any
    }
}

struct ByteBufDeserializer(std::vec::Vec<u8>);

impl<'de> Deserializer<'de> for ByteBufDeserializer {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_byte_buf(self.0)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_byte_buf(self.0)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct
        enum identifier ignored_any
    }
}

#[test]
fn primitive_and_option_adapters_cover_all_scalar_families() {
    let arena = Arena::new();

    macro_rules! json_value {
        ($ty:ty, $input:literal, $expected:expr) => {{
            let mut input = serde_json::Deserializer::from_str($input);
            let actual = <$ty as DeserializeIn<'_, _>>::deserialize_in(&arena, &mut input).unwrap();
            assert_eq!(actual, $expected);
        }};
    }

    json_value!((), "null", ());
    json_value!(bool, "true", true);
    json_value!(char, r#""x""#, 'x');
    json_value!(i8, "-1", -1);
    json_value!(i16, "-2", -2);
    json_value!(i32, "-3", -3);
    json_value!(i64, "-4", -4);
    json_value!(i128, "-5", -5);
    json_value!(isize, "-6", -6);
    json_value!(u8, "1", 1);
    json_value!(u16, "2", 2);
    json_value!(u32, "3", 3);
    json_value!(u64, "4", 4);
    json_value!(u128, "5", 5);
    json_value!(usize, "6", 6);
    json_value!(f32, "1.5", 1.5);
    json_value!(f64, "2.5", 2.5);
    json_value!(Option<u64>, "null", None);
    json_value!(Option<u64>, "7", Some(7));

    let none = <Option<u64> as DeserializeIn<'_, _>>::deserialize_in(&arena, UnitOptionDeserializer).unwrap();
    assert_eq!(none, None);

    let error = <Option<u64> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
        .unwrap_err();
    assert!(error.to_string().contains("optional"));
}

#[test]
fn string_and_slice_adapters_cover_all_pointer_kinds_and_callbacks() {
    let arena = Arena::new();

    let boxed = <Box<str> as DeserializeIn<'_, _>>::deserialize_in(&arena, BorrowedStrDeserializer::<ValueError>::new("borrowed")).unwrap();
    let shared =
        <Arc<str> as DeserializeIn<'_, _>>::deserialize_in(&arena, StringDeserializer::<ValueError>::new("owned".to_owned())).unwrap();
    let local =
        <Rc<str> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::StrDeserializer::<ValueError>::new("local")).unwrap();
    assert_eq!((&*boxed, &*shared, &*local), ("borrowed", "owned", "local"));

    let boxed_slice =
        <Box<[u64]> as DeserializeIn<'_, _>>::deserialize_in(&arena, SeqDeserializer::<_, ValueError>::new([1_u64, 2].into_iter()))
            .unwrap();
    let arc_slice =
        <Arc<[u64]> as DeserializeIn<'_, _>>::deserialize_in(&arena, SeqDeserializer::<_, ValueError>::new([3_u64, 4].into_iter()))
            .unwrap();
    let rc_slice =
        <Rc<[u64]> as DeserializeIn<'_, _>>::deserialize_in(&arena, SeqDeserializer::<_, ValueError>::new([5_u64, 6].into_iter())).unwrap();
    assert_eq!((&*boxed_slice, &*arc_slice, &*rc_slice), (&[1, 2][..], &[3, 4][..], &[5, 6][..]));

    let string_error =
        <Box<str> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
            .unwrap_err();
    assert!(string_error.to_string().contains("string"));

    let sequence_error =
        <Box<[u64]> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
            .unwrap_err();
    assert!(sequence_error.to_string().contains("sequence"));
    let arc_error =
        <Arc<[u64]> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
            .unwrap_err();
    assert!(arc_error.to_string().contains("sequence"));
    let rc_error = <Rc<[u64]> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
        .unwrap_err();
    assert!(rc_error.to_string().contains("sequence"));
}

#[test]
fn sized_pointer_and_root_adapters_cover_success_limit_and_allocation_errors() {
    let arena = Arena::new();

    let boxed = <Box<u64> as DeserializeIn<'_, _>>::deserialize_in(&arena, U64Deserializer::<ValueError>::new(1)).unwrap();
    let shared = <Arc<u64> as DeserializeIn<'_, _>>::deserialize_in(&arena, U64Deserializer::<ValueError>::new(2)).unwrap();
    let local = <Rc<u64> as DeserializeIn<'_, _>>::deserialize_in(&arena, U64Deserializer::<ValueError>::new(3)).unwrap();
    assert_eq!((*boxed, *shared, *local), (1, 2, 3));

    let mut input = serde_json::Deserializer::from_str("4");
    let value: Arc<u64> = arena.deserialize(&mut input).unwrap();
    assert_eq!(*value, 4);
    let mut input = serde_json::Deserializer::from_str("5");
    let value: Box<u64> = arena.deserialize(&mut input).unwrap();
    assert_eq!(*value, 5);
    let mut input = serde_json::Deserializer::from_str("6");
    let value: Rc<u64> = arena.deserialize(&mut input).unwrap();
    assert_eq!(*value, 6);

    let mut input = serde_json::Deserializer::from_str("7");
    let value: Arc<u64> = arena.deserialize_with_limits(&mut input, unlimited()).unwrap();
    assert_eq!(*value, 7);
    let mut input = serde_json::Deserializer::from_str("8");
    let value: Box<u64> = arena.deserialize_with_limits(&mut input, unlimited()).unwrap();
    assert_eq!(*value, 8);
    let mut input = serde_json::Deserializer::from_str("9");
    let value: Rc<u64> = arena.deserialize_with_limits(&mut input, unlimited()).unwrap();
    assert_eq!(*value, 9);

    let failing = Arena::new_in(FailingAllocator::new(0));
    assert!(<Box<u64, _> as DeserializeIn<'_, _>>::deserialize_in(&failing, U64Deserializer::<ValueError>::new(1)).is_err());
    assert!(<Rc<u64, _> as DeserializeIn<'_, _>>::deserialize_in(&failing, U64Deserializer::<ValueError>::new(1)).is_err());
    let sync_failing = Arena::new_in(SyncFailingAllocator::new(0));
    assert!(<Arc<u64, _> as DeserializeIn<'_, _>>::deserialize_in(&sync_failing, U64Deserializer::<ValueError>::new(1)).is_err());
}

#[test]
fn smart_pointer_serialization_covers_sized_and_slice_forms() {
    let arena = Arena::new();

    fn encoded<T: serde::Serialize + ?Sized>(value: &T) -> std::string::String {
        serde_json::to_string(value).unwrap()
    }

    assert_eq!(encoded(&arena.alloc_box(1_u64)), "1");
    assert_eq!(encoded(&arena.alloc_arc(2_u64)), "2");
    assert_eq!(encoded(&arena.alloc_rc(3_u64)), "3");
    assert_eq!(encoded(&arena.alloc_slice_copy_box([1_u64, 2])), "[1,2]");
    assert_eq!(encoded(&arena.alloc_slice_copy_arc([3_u64, 4])), "[3,4]");
    assert_eq!(encoded(&arena.alloc_slice_copy_rc([5_u64, 6])), "[5,6]");
}

#[test]
fn string_cow_covers_traits_serialization_and_all_input_callbacks() {
    let arena = Arena::new();
    let borrowed: Cow<'_, str> = Cow::Borrowed("borrowed");
    let owned: Cow<'_, str> = Cow::Owned(arena.alloc_str_box("owned"));

    assert_eq!(borrowed.as_ref(), "borrowed");
    assert_eq!(&*borrowed, "borrowed");
    assert_eq!(format!("{borrowed}"), "borrowed");
    assert_eq!(format!("{borrowed:?}"), "\"borrowed\"");
    assert_ne!(borrowed, owned);
    let same: Cow<'_, str> = Cow::Borrowed("same");
    assert_eq!(same, Cow::Borrowed("same"));
    assert_eq!(Cow::Borrowed("owned"), owned);
    assert_eq!(serde_json::to_string(&owned).unwrap(), r#""owned""#);
    assert_eq!(&*Cow::Borrowed("copied").try_into_owned(&arena).unwrap(), "copied");
    assert_eq!(&*owned.try_into_owned(&arena).unwrap(), "owned");

    let from_borrow: Cow<'_, str> = DeserializeIn::deserialize_in(&arena, BorrowedStrDeserializer::<ValueError>::new("direct")).unwrap();
    let from_string: Cow<'_, str> =
        DeserializeIn::deserialize_in(&arena, StringDeserializer::<ValueError>::new("temporary".to_owned())).unwrap();
    assert!(from_borrow.is_borrowed());
    assert_eq!(&*from_string, "temporary");

    let error = <Cow<'_, str> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
        .unwrap_err();
    assert!(error.to_string().contains("string"));

    let bytes: Cow<'_, [u8]> = DeserializeIn::deserialize_in(&arena, ByteBufDeserializer(vec![1, 2])).unwrap();
    assert_eq!(&*bytes, &[1, 2]);
    assert!(bytes.is_owned());
    let error =
        <Cow<'_, [u8]> as DeserializeIn<'_, _>>::deserialize_in(&arena, serde::de::value::BoolDeserializer::<ValueError>::new(true))
            .unwrap_err();
    assert!(error.to_string().contains("byte string"));
}

#[test]
fn reusable_deserialization_covers_callbacks_hints_errors_and_allocation_failure() {
    let arena = Arena::new();

    let mut text = arena.alloc_string_with_capacity(16);
    text.deserialize_reusing(BorrowedStrDeserializer::<ValueError>::new("borrowed"))
        .unwrap();
    assert_eq!(text.as_str(), "borrowed");
    text.deserialize_reusing(StringDeserializer::<ValueError>::new("owned".to_owned()))
        .unwrap();
    assert_eq!(text.as_str(), "owned");
    let error = text
        .deserialize_reusing(serde::de::value::BoolDeserializer::<ValueError>::new(true))
        .unwrap_err();
    assert!(error.to_string().contains("string"));

    let mut values: multitude::vec::Vec<'_, u64> = arena.alloc_vec_with_capacity(4);
    values
        .deserialize_reusing(SeqDeserializer::<_, ValueError>::new([1_u64, 2, 3].into_iter()))
        .unwrap();
    assert_eq!(values.as_slice(), &[1, 2, 3]);
    let error = values
        .deserialize_reusing(serde::de::value::BoolDeserializer::<ValueError>::new(true))
        .unwrap_err();
    assert!(error.to_string().contains("sequence"));

    let failing = Arena::new_in(FailingAllocator::new(0));
    let mut text = failing.alloc_string();
    assert!(
        text.deserialize_reusing(StringDeserializer::<ValueError>::new("allocation".to_owned()))
            .is_err()
    );
    let mut values = failing.alloc_vec::<u64>();
    assert!(
        values
            .deserialize_reusing(SeqDeserializer::<_, ValueError>::new([1_u64].into_iter()))
            .is_err()
    );
}

#[test]
fn json_convenience_methods_cover_root_inference_inputs_and_limits() {
    let arena = Arena::new();

    let shared: Arc<u64> = arena.deserialize_json("1").unwrap();
    let boxed: Box<u64> = arena.deserialize_json(b"2").unwrap();
    let local: Rc<u64> = arena.deserialize_json(&b"3"[..]).unwrap();
    assert_eq!((*shared, *boxed, *local), (1, 2, 3));

    let shared: Arc<u64> = arena.deserialize_json_with_limits("4", unlimited()).unwrap();
    let boxed: Box<u64> = arena.deserialize_json_with_limits(b"5", unlimited()).unwrap();
    let local: Rc<u64> = arena.deserialize_json_with_limits(&b"6"[..], unlimited()).unwrap();
    assert_eq!((*shared, *boxed, *local), (4, 5, 6));

    fn trailing<T: Debug>(result: serde_json::Result<T>) {
        assert!(result.unwrap_err().to_string().contains("trailing characters"));
    }

    trailing(arena.deserialize_json::<Box<u64>, _>("1 2"));
    let error = arena.deserialize_json_with_limits::<Box<u64>, _>(b"1 2", unlimited()).unwrap_err();
    assert!(error.as_json_error().to_string().contains("trailing characters"));
}

#[test]
fn ordinary_serde_seed_default_and_deserialize_paths_are_covered() {
    let seed = multitude::de::DeserializeSeed::<u64>::default();
    assert_eq!(seed.deserialize(U64Deserializer::<ValueError>::new(42)).unwrap(), 42);

    let arena = Arena::new();
    let seed = DeserializeInSeed::<u64, _>::new(&arena);
    assert_eq!(seed.deserialize(U64Deserializer::<ValueError>::new(43)).unwrap(), 43);
}
