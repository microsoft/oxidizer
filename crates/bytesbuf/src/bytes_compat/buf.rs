// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::BufMut;
use bytes::buf::UninitSlice;

use crate::BytesBuf;

// SAFETY: The trait documentation does not define any safety requirements we need to fulfill.
// It is unclear why the trait is marked unsafe in the first place.
unsafe impl BufMut for BytesBuf {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn remaining_mut(&self) -> usize {
        self.remaining_capacity()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    unsafe fn advance_mut(&mut self, cnt: usize) {
        // SAFETY: Forwarding safety requirements to the caller.
        unsafe {
            self.advance(cnt);
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn chunk_mut(&mut self) -> &mut UninitSlice {
        UninitSlice::uninit(self.first_unfilled_slice())
    }
}
