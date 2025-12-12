// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::IoSlice;

use bytes::Buf;

use crate::BytesView;

impl Buf for BytesView {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn remaining(&self) -> usize {
        self.len()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn chunk(&self) -> &[u8] {
        self.first_slice()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        self.io_slices(dst)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn advance(&mut self, cnt: usize) {
        self.advance(cnt);
    }
}
