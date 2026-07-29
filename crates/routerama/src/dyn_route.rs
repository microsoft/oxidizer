// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;

/// A dynamic route's extractor and its template-to-variant capture permutation.
#[derive(Debug)]
pub struct DynRoute<X> {
    extractor: X,
    capture_order: Box<[usize]>,
}

impl<X> DynRoute<X> {
    pub(crate) fn new(extractor: X, capture_order: Box<[usize]>) -> Self {
        Self { extractor, capture_order }
    }

    /// The generated extractor attached to this route.
    pub fn extractor(&self) -> &X {
        &self.extractor
    }

    /// Maps generated field order to the matched template's capture order.
    pub fn capture_order(&self) -> &[usize] {
        &self.capture_order
    }
}
