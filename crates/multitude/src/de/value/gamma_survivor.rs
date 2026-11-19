// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::ToString;
use core::cell::Cell;
use core::marker::PhantomData;

use allocator_api2::alloc::AllocError as BackingError;
use serde::de::value::{Error, U8Deserializer};

use super::*;

#[derive(Clone, Copy, Debug)]
struct Failing;

// SAFETY: this allocator never returns an allocation.
unsafe impl Allocator for Failing {
    fn allocate(&self, _layout: core::alloc::Layout) -> Result<core::ptr::NonNull<[u8]>, BackingError> {
        Err(BackingError)
    }
    unsafe fn deallocate(&self, _pointer: core::ptr::NonNull<u8>, _layout: core::alloc::Layout) {
        panic!("no allocation can have come from Failing")
    }
}

struct Sequence<'a> {
    calls: &'a Cell<usize>,
    remaining: bool,
    hint: Option<usize>,
}

impl<'de> SeqAccess<'de> for Sequence<'_> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        self.calls.set(self.calls.get() + 1);
        if core::mem::replace(&mut self.remaining, false) {
            seed.deserialize(U8Deserializer::<Error>::new(7)).map(Some)
        } else {
            Ok(None)
        }
    }
    fn size_hint(&self) -> Option<usize> {
        self.hint
    }
}

struct Mapping<'a> {
    keys: &'a Cell<usize>,
    values: &'a Cell<usize>,
    remaining: bool,
    fail_value: bool,
    hint: Option<usize>,
}

impl<'de> MapAccess<'de> for Mapping<'_> {
    type Error = Error;

    fn next_key_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        self.keys.set(self.keys.get() + 1);
        if core::mem::replace(&mut self.remaining, false) {
            seed.deserialize(U8Deserializer::<Error>::new(1)).map(Some)
        } else {
            Ok(None)
        }
    }
    fn next_value_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value, Error> {
        self.values.set(self.values.get() + 1);
        if self.fail_value {
            Err(Error::custom("gamma map value failure"))
        } else {
            seed.deserialize(U8Deserializer::<Error>::new(7))
        }
    }
    fn size_hint(&self) -> Option<usize> {
        self.hint
    }
}

#[test]
fn gamma_survivor_value_sequence_and_map_error_stage_and_zero_hints() {
    let arena = Arena::new_in(Failing);
    for (hint, remaining, expected) in [(None, false, 1), (None, true, 1), (Some(1), true, 0), (Some(usize::MAX), true, 0)] {
        let calls = Cell::new(0);
        gamma_survivor_expect_error(ValueVisitor { arena: &arena }.visit_seq(Sequence {
            calls: &calls,
            remaining,
            hint,
        }));
        assert_eq!(calls.get(), expected);
    }
    for (hint, remaining, expected) in [(None, false, 1), (None, true, 1), (Some(1), true, 0), (Some(usize::MAX), true, 0)] {
        let keys = Cell::new(0);
        let values = Cell::new(0);
        gamma_survivor_expect_error(ValueVisitor { arena: &arena }.visit_map(Mapping {
            keys: &keys,
            values: &values,
            remaining,
            fail_value: false,
            hint,
        }));
        assert_eq!(keys.get(), expected);
        assert_eq!(values.get(), usize::from(remaining && expected != 0));
    }
    let keys = Cell::new(0);
    let values = Cell::new(0);
    let error = ValueVisitor { arena: &arena }
        .visit_map(Mapping {
            keys: &keys,
            values: &values,
            remaining: true,
            fail_value: true,
            hint: None,
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "gamma map value failure");
    assert_eq!(keys.get(), 1);
    assert_eq!(values.get(), 1);
}

#[test]
fn gamma_survivor_value_boolean_capture_and_exact_byte_reservation() {
    let arena = Arena::new();
    for value in [false, true] {
        let captured: Value = Value::deserialize_in(&arena, serde::de::value::BoolDeserializer::<Error>::new(value)).unwrap();
        assert_eq!(captured.as_bool(), Some(value));
    }
    let bytes = ValueVisitor { arena: &arena }.visit_borrowed_bytes::<Error>(&[1, 2, 3]).unwrap();
    assert_eq!(bytes.as_bytes(), Some(&[1, 2, 3][..]));
    let pointer = bytes.as_bytes().unwrap().as_ptr();
    let next = arena.alloc(7_u8);
    assert_eq!((&raw const *next).addr(), pointer.addr() + 3);
}

#[test]
fn gamma_survivor_map_replay_requires_key_and_enum_seed_error_propagates() {
    let entries: [Entry; 1] = [Entry {
        key: Value::Number(Number::U8(1)),
        value: Value::Number(Number::U8(7)),
    }];
    let mut replay = MapReplay::new(&entries);
    assert_eq!(
        replay.next_value_seed(PhantomData::<u8>).unwrap_err().to_string(),
        "value requested before key"
    );
    assert_eq!(replay.next_key_seed(PhantomData::<u8>).unwrap(), Some(1));
    assert_eq!(replay.next_value_seed(PhantomData::<u8>).unwrap(), 7);
    assert_eq!(
        replay.next_value_seed(PhantomData::<u8>).unwrap_err().to_string(),
        "value requested before key"
    );
    let replay = EnumReplay::<allocator_api2::alloc::Global>::unit("Variant");
    assert_eq!(
        replay.variant_seed(PhantomData::<u8>).err().unwrap().to_string(),
        "invalid type: string \"Variant\", expected u8"
    );
}

#[cfg(test)]
fn gamma_survivor_expect_error<T, E>(result: Result<T, E>) {
    let Err(_) = result else {
        panic!("the fixture requires this operation to return its injected error");
    };
    drop(result);
}
