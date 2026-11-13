// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "std")]
use std::io::IoSlice;

use bytes::Buf;

use crate::BytesView;

impl Buf for BytesView {
    fn remaining(&self) -> usize {
        self.len()
    }

    fn chunk(&self) -> &[u8] {
        self.first_slice()
    }

    #[cfg(feature = "std")]
    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        let slice_count = self.spans_reversed.len().min(dst.len());

        for (i, span) in self.spans_reversed.iter().rev().take(slice_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = IoSlice::new(span);
        }

        slice_count
    }

    fn advance(&mut self, cnt: usize) {
        self.advance(cnt);
    }
}
