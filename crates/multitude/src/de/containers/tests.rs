// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::assertions_on_result_states, reason = "tests deliberately exercise error branches")]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::cell::{Cell, RefCell};
use core::cmp::Reverse;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};

use serde::de::value::{BoolDeserializer, Error, MapDeserializer, U8Deserializer, UnitDeserializer};
use serde::de::{DeserializeSeed, Deserializer, EnumAccess, Error as _, IntoDeserializer, SeqAccess, VariantAccess, Visitor};
use serde::forward_to_deserialize_any;

use super::DeserializeIn;
use crate::Arena;

#[test]
fn overlong_array_diagnostic_length_saturates() {
    assert_eq!(super::overlong_array_length(4), 5);
    assert_eq!(super::overlong_array_length(usize::MAX), usize::MAX);
}

#[test]
fn vec_reserves_only_when_capacity_is_exhausted() {
    assert!(super::vec_needs_reserve(0, 0));
    assert!(super::vec_needs_reserve(4, 4));
    assert!(!super::vec_needs_reserve(0, 4));
    assert!(!super::vec_needs_reserve(3, 4));
}

fn from_json<T>(input: &str) -> Result<T, serde_json::Error>
where
    for<'de> T: DeserializeIn<'de, allocator_api2::alloc::Global>,
{
    let arena = Arena::new();
    let mut deserializer = serde_json::Deserializer::from_str(input);
    T::deserialize_in(&arena, &mut deserializer)
}

enum Identifier {
    Number(u64),
    String(&'static str),
    Bytes(&'static [u8]),
}

struct IdentifierDeserializer(Identifier);

impl<'de> Deserializer<'de> for IdentifierDeserializer {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_identifier(visitor)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Identifier::Number(value) => visitor.visit_u64(value),
            Identifier::String(value) => visitor.visit_borrowed_str(value),
            Identifier::Bytes(value) => visitor.visit_borrowed_bytes(value),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum ignored_any
    }
}

struct ResultDeserializer(Identifier);

impl<'de> Deserializer<'de> for ResultDeserializer {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_enum("Result", &["Ok", "Err"], visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_enum(ResultAccess(self.0))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct identifier ignored_any
    }
}

struct ResultAccess(Identifier);

impl<'de> EnumAccess<'de> for ResultAccess {
    type Error = Error;
    type Variant = Payload;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error> {
        seed.deserialize(IdentifierDeserializer(self.0)).map(|field| (field, Payload))
    }
}

struct Payload;

impl<'de> VariantAccess<'de> for Payload {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Err(Error::custom("not unit"))
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Self::Error> {
        seed.deserialize(7_u8.into_deserializer())
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(Error::custom("not tuple"))
    }

    fn struct_variant<V: Visitor<'de>>(self, _fields: &'static [&'static str], _visitor: V) -> Result<V::Value, Self::Error> {
        Err(Error::custom("not struct"))
    }
}

struct TestSequence {
    values: alloc::vec::IntoIter<Result<u8, Error>>,
    hint: Option<usize>,
}

impl TestSequence {
    fn new(values: Vec<Result<u8, Error>>, hint: Option<usize>) -> Self {
        Self {
            values: values.into_iter(),
            hint,
        }
    }
}

impl<'de> SeqAccess<'de> for TestSequence {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        self.values
            .next()
            .transpose()?
            .map(|value| seed.deserialize(value.into_deserializer()))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        self.hint
    }
}

struct SequenceDeserializer(TestSequence);

impl<'de> Deserializer<'de> for SequenceDeserializer {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(self.0)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(self.0)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(self.0)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct tuple_struct map
        struct enum identifier ignored_any
    }
}

fn sequence(values: &[u8], hint: Option<usize>) -> SequenceDeserializer {
    SequenceDeserializer(TestSequence::new(values.iter().copied().map(Ok).collect(), hint))
}

#[test]
fn phantom_data_and_result_identifiers() {
    let arena = Arena::new();
    let value = PhantomData::<u8>::deserialize_in(&arena, UnitDeserializer::<Error>::new()).unwrap();
    assert_eq!(value, PhantomData);

    for identifier in [Identifier::Number(0), Identifier::String("Ok"), Identifier::Bytes(b"Ok")] {
        assert_eq!(
            Result::<u8, u8>::deserialize_in(&arena, ResultDeserializer(identifier)).unwrap(),
            Ok(7)
        );
    }
    for identifier in [Identifier::Number(1), Identifier::String("Err"), Identifier::Bytes(b"Err")] {
        assert_eq!(
            Result::<u8, u8>::deserialize_in(&arena, ResultDeserializer(identifier)).unwrap(),
            Err(7)
        );
    }

    for (identifier, expected) in [
        (Identifier::Number(2), "expected `Ok` or `Err`"),
        (Identifier::String("Other"), "unknown variant"),
        (Identifier::Bytes(b"Other"), "unknown variant"),
        (Identifier::Bytes(b"\xff"), "expected `Ok` or `Err`"),
    ] {
        let error = Result::<u8, u8>::deserialize_in(&arena, ResultDeserializer(identifier)).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

macro_rules! assert_tuple {
    ($type:ty, $values:expr) => {{
        let arena = Arena::new();
        <$type>::deserialize_in(&arena, sequence($values, Some($values.len()))).unwrap()
    }};
}

#[test]
fn every_tuple_arity_and_missing_elements() {
    assert_eq!(assert_tuple!((u8,), &[0]), (0,));
    assert_eq!(assert_tuple!((u8, u8), &[0, 1]), (0, 1));
    assert_eq!(assert_tuple!((u8, u8, u8), &[0, 1, 2]), (0, 1, 2));
    assert_tuple!((u8, u8, u8, u8), &[0, 1, 2, 3]);
    assert_tuple!((u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4]);
    assert_tuple!((u8, u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4, 5]);
    assert_tuple!((u8, u8, u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4, 5, 6]);
    assert_tuple!((u8, u8, u8, u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4, 5, 6, 7]);
    assert_tuple!((u8, u8, u8, u8, u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_tuple!((u8, u8, u8, u8, u8, u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_tuple!((u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    assert_tuple!(
        (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
    );
    assert_tuple!(
        (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_tuple!(
        (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]
    );
    assert_tuple!(
        (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
    );
    assert_tuple!(
        (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    );

    let arena = Arena::new();
    for length in 0..3 {
        let values: Vec<_> = (0..length).map(Ok).collect();
        let error = <(u8, u8, u8)>::deserialize_in(&arena, SequenceDeserializer(TestSequence::new(values, None))).unwrap_err();
        assert!(error.to_string().contains(&format!("invalid length {length}")));
    }
}

static DROPS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct DropProbe(u8);

impl Drop for DropProbe {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

impl<'de, A: allocator_api2::alloc::Allocator + Clone> DeserializeIn<'de, A> for DropProbe {
    fn deserialize_in<D: Deserializer<'de>>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error> {
        u8::deserialize_in(arena, deserializer).map(Self)
    }
}

#[test]
fn fixed_arrays_success_lengths_and_drop_safety() {
    let arena = Arena::new();
    let empty = <[u8; 0]>::deserialize_in(&arena, sequence(&[], Some(0))).unwrap();
    assert_eq!(empty, [0_u8; 0]);
    assert_eq!(<[u8; 3]>::deserialize_in(&arena, sequence(&[1, 2, 3], Some(3))).unwrap(), [1, 2, 3]);

    let short = <[u8; 3]>::deserialize_in(&arena, sequence(&[1, 2], None)).unwrap_err();
    assert!(short.to_string().contains("invalid length 2"));
    let long = <[u8; 2]>::deserialize_in(&arena, sequence(&[1, 2, 3], None)).unwrap_err();
    assert!(long.to_string().contains("invalid length 3"));

    DROPS.store(0, Ordering::Relaxed);
    let input = SequenceDeserializer(TestSequence::new(vec![Ok(1), Err(Error::custom("element failed"))], None));
    assert!(<[DropProbe; 3]>::deserialize_in(&arena, input).is_err());
    assert_eq!(DROPS.load(Ordering::Relaxed), 1);

    let probes = <[DropProbe; 2]>::deserialize_in(&arena, sequence(&[4, 5], None)).unwrap();
    assert_eq!((probes[0].0, probes[1].0), (4, 5));
    drop(probes);
    assert_eq!(DROPS.load(Ordering::Relaxed), 3);

    let long = <[DropProbe; 2]>::deserialize_in(&arena, sequence(&[6, 7, 8], None)).unwrap_err();
    assert!(long.to_string().contains("invalid length 3"));
    assert_eq!(DROPS.load(Ordering::Relaxed), 5);
}

#[test]
fn vec_hints_growth_errors_and_wrappers() {
    let arena = Arena::new();
    let hinted = Vec::<u8>::deserialize_in(&arena, sequence(&[1, 2, 3], Some(3))).unwrap();
    assert_eq!(hinted, [1, 2, 3]);
    let grown = Vec::<u8>::deserialize_in(&arena, sequence(&[1, 2, 3, 4, 5], Some(0))).unwrap();
    assert_eq!(grown, [1, 2, 3, 4, 5]);
    let no_hint = Vec::<u8>::deserialize_in(&arena, sequence(&[9], None)).unwrap();
    assert_eq!(no_hint, [9]);
    let element_error = SequenceDeserializer(TestSequence::new(vec![Ok(1), Err(Error::custom("bad"))], None));
    assert!(Vec::<u8>::deserialize_in(&arena, element_error).is_err());
    let reserve_error = Vec::<u8>::deserialize_in(&arena, sequence(&[], Some(usize::MAX))).unwrap_err();
    assert!(!reserve_error.to_string().is_empty());

    assert_eq!(
        Cell::<u8>::deserialize_in(&arena, U8Deserializer::<Error>::new(3)).unwrap().get(),
        3
    );
    assert_eq!(
        *RefCell::<u8>::deserialize_in(&arena, U8Deserializer::<Error>::new(4))
            .unwrap()
            .borrow(),
        4
    );
    assert_eq!(
        Reverse::<u8>::deserialize_in(&arena, U8Deserializer::<Error>::new(5)).unwrap(),
        Reverse(5)
    );
}

#[test]
fn btree_collections_and_expecting_errors() {
    let arena = Arena::new();
    let map_input = MapDeserializer::<_, Error>::new([(2_u8, 20_u8), (1, 10), (2, 21)].into_iter());
    let map = BTreeMap::<u8, u8>::deserialize_in(&arena, map_input).unwrap();
    assert_eq!(map, BTreeMap::from([(1, 10), (2, 21)]));

    let set = BTreeSet::<u8>::deserialize_in(&arena, sequence(&[3, 1, 3, 2], None)).unwrap();
    assert_eq!(set, BTreeSet::from([1, 2, 3]));
    assert!(from_json::<BTreeMap<u8, u8>>(r#"{"1":"bad"}"#).is_err());
    assert!(from_json::<BTreeSet<u8>>(r#"[1,"bad"]"#).is_err());

    assert!(from_json::<PhantomData<u8>>("false").unwrap_err().to_string().contains("unit"));
    let result_error = Result::<u8, u8>::deserialize_in(&arena, BoolDeserializer::<Error>::new(false)).unwrap_err();
    assert!(result_error.to_string().contains("enum Result"));
    assert!(from_json::<(u8, u8)>("[1]").unwrap_err().to_string().contains("tuple of size 2"));
    assert!(from_json::<[u8; 2]>("[1]").unwrap_err().to_string().contains("array of length 2"));
    assert!(from_json::<Vec<u8>>("false").unwrap_err().to_string().contains("a sequence"));
    assert!(from_json::<BTreeMap<u8, u8>>("false").unwrap_err().to_string().contains("a map"));
    assert!(from_json::<BTreeSet<u8>>("false").unwrap_err().to_string().contains("a sequence"));
}
