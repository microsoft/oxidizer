// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::ToString;

use super::*;

struct LengthSpy<'a>(&'a Cell<usize>);

impl<'de> VariantAccess<'de> for LengthSpy<'_> {
    type Error = serde::de::value::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Err(serde::de::Error::custom("unit"))
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, _seed: T) -> Result<T::Value, Self::Error> {
        Err(serde::de::Error::custom("newtype"))
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, _visitor: V) -> Result<V::Value, Self::Error> {
        self.0.set(len);
        Err(serde::de::Error::custom("tuple length recorded"))
    }
    fn struct_variant<V: Visitor<'de>>(self, _fields: &'static [&'static str], _visitor: V) -> Result<V::Value, Self::Error> {
        Err(serde::de::Error::custom("struct"))
    }
}

struct UnitVisitor;

impl Visitor<'_> for UnitVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unit")
    }
}

#[test]
fn gamma_survivor_limited_initial_counts_depth_hints_and_tuple_length() {
    use serde::de::value::{Error, MapDeserializer, SeqDeserializer};

    let limits = DeserializationLimits::unlimited()
        .with_max_depth(1)
        .with_max_sequence_len(1)
        .with_max_map_len(1);
    let (result, detail) = deserialize_seed_with_limits_detailed(
        SeqDeserializer::<_, Error>::new([7_u8].into_iter()),
        PhantomData::<alloc::vec::Vec<u8>>,
        limits,
    );
    assert_eq!(result.unwrap(), [7]);
    assert_eq!(detail, None);
    let (result, detail) = deserialize_seed_with_limits_detailed(
        MapDeserializer::<_, Error>::new([(1_u8, 2_u8)].into_iter()),
        PhantomData::<alloc::collections::BTreeMap<u8, u8>>,
        limits,
    );
    assert_eq!(result.unwrap().get(&1), Some(&2));
    assert_eq!(detail, None);
    let state = LimitState {
        limits: limits.with_max_sequence_len(3).with_max_map_len(3),
        depth: Cell::new(0),
        limit_exceeded: Cell::new(None),
    };
    let mut sequence = LimitedSeqAccess {
        inner: SeqDeserializer::<_, Error>::new([7_u8].into_iter()),
        state: &state,
        count: 0,
    };
    assert_eq!(sequence.next_element_seed(PhantomData::<u8>).unwrap(), Some(7));
    assert_eq!(sequence.count, 1);
    assert_eq!(sequence.next_element_seed(PhantomData::<u8>).unwrap(), None);
    assert_eq!(sequence.count, 1);
    let mut mapping = LimitedMapAccess {
        inner: MapDeserializer::<_, Error>::new([(1_u8, 2_u8)].into_iter()),
        state: &state,
        count: 0,
    };
    assert_eq!(mapping.next_key_seed(PhantomData::<u8>).unwrap(), Some(1));
    assert_eq!(mapping.count, 1);
    assert_eq!(mapping.next_value_seed(PhantomData::<u8>).unwrap(), 2);
    assert_eq!(mapping.next_key_seed(PhantomData::<u8>).unwrap(), None);
    assert_eq!(mapping.count, 1);
    let length = Cell::new(usize::MAX);
    let error = LimitedVariantAccess {
        inner: LengthSpy(&length),
        state: &state,
    }
    .tuple_variant(3, UnitVisitor)
    .unwrap_err();
    assert_eq!(length.get(), 3);
    assert_eq!(error.to_string(), "tuple length recorded");
}

#[test]
fn gamma_survivor_limit_depth_guards_and_first_error() {
    let state = LimitState {
        limits: DeserializationLimits::unlimited().with_max_depth(2),
        depth: Cell::new(0),
        limit_exceeded: Cell::new(None),
    };
    let first = state.enter_guard::<serde::de::value::Error>().unwrap();
    assert_eq!(state.depth.get(), 1);
    {
        let second = state.enter_guard::<serde::de::value::Error>().unwrap();
        assert_eq!(state.depth.get(), 2);
        let error = state.enter_guard::<serde::de::value::Error>().err().unwrap();
        assert_eq!(error.to_string(), "deserialization nesting depth limit exceeded");
        assert_eq!(state.depth.get(), 2);
        assert_eq!(
            state.limit_exceeded.get(),
            Some(LimitExceeded::new(DeserializationResource::Depth, 2))
        );
        drop(second);
        assert_eq!(state.depth.get(), 1);
    }
    let later = state.reject::<serde::de::value::Error>(DeserializationResource::StringLength, 9, "later error");
    assert_eq!(later.to_string(), "later error");
    assert_eq!(
        state.limit_exceeded.get(),
        Some(LimitExceeded::new(DeserializationResource::Depth, 2))
    );
    drop(first);
    assert_eq!(state.depth.get(), 0);
    let recovered = state.enter_guard::<serde::de::value::Error>().unwrap();
    assert_eq!(state.depth.get(), 1);
    drop(recovered);
    assert_eq!(state.depth.get(), 0);
}

#[test]
fn gamma_survivor_unlimited_depth_does_not_mutate_counter() {
    let state = LimitState {
        limits: DeserializationLimits::unlimited(),
        depth: Cell::new(usize::MAX),
        limit_exceeded: Cell::new(None),
    };
    let guard = state.enter_guard::<serde::de::value::Error>().unwrap();
    assert_eq!(state.depth.get(), usize::MAX);
    drop(guard);
    assert_eq!(state.depth.get(), usize::MAX);
    assert_eq!(state.limit_exceeded.get(), None);
}
