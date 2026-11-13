// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;

use allocator_api2::alloc::Allocator;
use serde::de::{Error as _, SeqAccess};

use super::{DeserializeIn, DeserializeInSeed};
use crate::Arena;

pub(super) struct SliceVisitor<'a, T, A: Allocator + Clone> {
    arena: &'a Arena<A>,
    marker: PhantomData<fn() -> T>,
}

impl<'a, T, A: Allocator + Clone> SliceVisitor<'a, T, A> {
    pub(super) const fn new(arena: &'a Arena<A>) -> Self {
        Self {
            arena,
            marker: PhantomData,
        }
    }
}

impl<'de, 'a, T, A> SliceVisitor<'a, T, A>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    pub(super) fn deserialize_vec<S>(self, mut sequence: S) -> Result<crate::vec::Vec<'a, T, A>, S::Error>
    where
        S: SeqAccess<'de>,
    {
        let mut values = self
            .arena
            .try_alloc_vec_with_capacity(sequence.size_hint().unwrap_or(0))
            .map_err(S::Error::custom)?;
        while let Some(value) = sequence.next_element_seed(DeserializeInSeed::<T, A>::new(self.arena))? {
            values.try_push(value).map_err(S::Error::custom)?;
        }
        Ok(values)
    }
}

#[cfg(test)]
mod gamma_survivor_tests {
    use alloc::string::ToString;
    use core::alloc::Layout;
    use core::cell::Cell;
    use core::ptr::NonNull;

    use serde::de::DeserializeSeed;

    use super::*;

    #[derive(Clone, Copy)]
    struct Reject;

    // SAFETY: this allocator always returns Err and never creates an allocation.
    unsafe impl Allocator for Reject {
        fn allocate(&self, _layout: Layout) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
            Err(allocator_api2::alloc::AllocError)
        }

        unsafe fn deallocate(&self, _ptr: NonNull<u8>, _layout: Layout) {
            panic!("Reject never returns an allocation to deallocate");
        }
    }

    struct Elements<'a> {
        hint: Option<usize>,
        remaining: bool,
        calls: &'a Cell<usize>,
    }

    impl<'de> SeqAccess<'de> for Elements<'_> {
        type Error = serde::de::value::Error;

        fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
            self.calls.set(self.calls.get() + 1);
            if core::mem::replace(&mut self.remaining, false) {
                seed.deserialize(serde::de::value::U8Deserializer::<Self::Error>::new(7)).map(Some)
            } else {
                Ok(None)
            }
        }

        fn size_hint(&self) -> Option<usize> {
            self.hint
        }
    }

    #[test]
    fn gamma_survivor_slice_visitor_absent_hint_and_allocation_error_stages() {
        let arena = Arena::new_in(Reject);
        let calls = Cell::new(0);
        let empty = SliceVisitor::<u8, _>::new(&arena)
            .deserialize_vec(Elements {
                hint: None,
                remaining: false,
                calls: &calls,
            })
            .expect("an empty sequence with no hint reserves no storage");
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.capacity(), 0);
        assert_eq!(calls.get(), 1);
        let expected = arena.try_alloc(0_u8).expect_err("Reject rejects every nonzero request").to_string();
        for (hint, expected_calls) in [(Some(1), 0), (None, 1)] {
            calls.set(0);
            let error = SliceVisitor::<u8, _>::new(&arena)
                .deserialize_vec(Elements {
                    hint,
                    remaining: true,
                    calls: &calls,
                })
                .expect_err("Reject cannot supply hinted capacity or grow after the first element");
            assert_eq!(error.to_string(), expected);
            assert_eq!(calls.get(), expected_calls);
        }
    }
}
