// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use core::marker::PhantomData;

use allocator_api2::alloc::Allocator;
use serde::de::{self, Visitor};

use crate::Arena;

pub(super) struct StrVisitor<'a, A: Allocator + Clone, R, F> {
    arena: &'a Arena<A>,
    allocate: F,
    marker: PhantomData<fn() -> R>,
}

impl<'a, A: Allocator + Clone, R, F> StrVisitor<'a, A, R, F> {
    pub(super) const fn new(arena: &'a Arena<A>, allocate: F) -> Self {
        Self {
            arena,
            allocate,
            marker: PhantomData,
        }
    }
}

impl<'de, A, R, F> Visitor<'de> for StrVisitor<'_, A, R, F>
where
    A: Allocator + Clone,
    F: Fn(&Arena<A>, &str) -> Result<R, crate::AllocError>,
{
    type Value = R;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        (self.allocate)(self.arena, v).map_err(E::custom)
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
        self.visit_str(v)
    }

    fn visit_string<E: de::Error>(self, v: alloc::string::String) -> Result<Self::Value, E> {
        self.visit_str(&v)
    }
}
