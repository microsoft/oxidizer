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
