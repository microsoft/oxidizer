// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::slice;

use crate::{FieldValue, validate};

// Concrete state gives IntoIterator a nameable type without boxing a closure-based iterator.
pub(super) struct CorsTokens<'a> {
    values: slice::Iter<'a, FieldValue>,
    remaining: &'a [u8],
}

impl<'a> CorsTokens<'a> {
    pub(super) const fn new(values: slice::Iter<'a, FieldValue>) -> Self {
        Self { values, remaining: &[] }
    }
}

impl<'a> Iterator for CorsTokens<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.remaining.is_empty() {
                self.remaining = self.values.next()?.as_bytes();
            }
            let end = self.remaining.iter().position(|byte| *byte == b',').unwrap_or(self.remaining.len());
            let (item, rest) = self.remaining.split_at(end);
            self.remaining = rest.get(1..).unwrap_or_default();
            let item = validate::trim_ows(item);
            if !item.is_empty() {
                return Some(item);
            }
        }
    }
}
