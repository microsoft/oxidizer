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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn buf_mut_compat() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        assert_eq!(buf.remaining_mut(), 100);

        // 100 + 100
        buf.reserve(200, &memory);

        assert_eq!(buf.remaining_mut(), 200);

        let chunk = buf.chunk_mut();
        assert_eq!(chunk.len(), 100);

        // SAFETY: Lies - we did not write anything. But we will also
        // not touch the data - we are only inspecting the bookkeeping.
        // Good enough for test code.
        unsafe {
            buf.advance_mut(50);
        }

        let chunk = buf.chunk_mut();
        assert_eq!(chunk.len(), 50);

        // SAFETY: See above.
        unsafe {
            buf.advance_mut(50);
        }

        let chunk = buf.chunk_mut();
        assert_eq!(chunk.len(), 100);

        // SAFETY: See above.
        unsafe {
            buf.advance_mut(100);
        }

        let chunk = buf.chunk_mut();
        assert_eq!(chunk.len(), 0);
    }
}
