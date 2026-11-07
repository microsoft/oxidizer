// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test-binary-local, one-shot failures of standard Vec's fallible growth.

#![cfg(feature = "serde")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use multitude::Arena;
use multitude::de::DeserializeIn;
use serde::de::value::{Error, U8Deserializer};
use serde::de::{DeserializeSeed, Deserializer, SeqAccess, Visitor};

thread_local! {
    static FAIL_NEXT: Cell<bool> = const { Cell::new(false) };
}

struct OneShotAllocator;

fn fail_requested() -> bool {
    FAIL_NEXT.try_with(|flag| flag.replace(false)).unwrap_or(false)
}

// SAFETY: successful operations are forwarded to System with unchanged
// arguments. Requested failures return null without taking or releasing storage.
unsafe impl GlobalAlloc for OneShotAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail_requested() {
            return core::ptr::null_mut();
        }
        // SAFETY: the caller supplies a valid nonzero allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if fail_requested() {
            return core::ptr::null_mut();
        }
        // SAFETY: the caller supplies a valid nonzero allocation layout.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if fail_requested() {
            return core::ptr::null_mut();
        }
        // SAFETY: the caller supplies the original layout and valid new size.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout identify an allocation forwarded to System.
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: OneShotAllocator = OneShotAllocator;

struct FailGrowthSequence;

impl<'de> SeqAccess<'de> for FailGrowthSequence {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        let value = seed.deserialize(U8Deserializer::<Error>::new(7))?;
        FAIL_NEXT.with(|flag| flag.set(true));
        Ok(Some(value))
    }
}

impl<'de> Deserializer<'de> for FailGrowthSequence {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_seq(self)
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

#[test]
fn gamma_survivor_standard_vec_growth_returns_allocator_error_not_abort() {
    let arena = Arena::new();
    let result = Vec::<u8>::deserialize_in(&arena, FailGrowthSequence);
    assert!(!FAIL_NEXT.with(Cell::get));
    let error = result.unwrap_err();
    assert_eq!(
        error.to_string(),
        "memory allocation failed because the memory allocator returned an error"
    );
}
