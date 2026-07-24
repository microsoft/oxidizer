// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`FlatReader`]: all strings in one contiguous, dense-indexed layout.
//!
//! Resolves handles through the crate's [`storage`](crate::storage) helpers, which
//! own the (single) unchecked-UTF-8 conversion — this module itself is
//! `unsafe`-free.

use alloc::boxed::Box;

use crate::reader::Reader;
use crate::sym::Sym;

/// A frozen, flat interner: every string concatenated in `buffer`, with CSR-style
/// `offsets` — `offsets[i]` the start and `offsets[i+1]` the end of the `i`-th
/// string (leading `0` sentinel, `len() + 1` entries). Indexed by a dense 0-based
/// index — one slice, no shards, no atomics, no per-index branch. Returned as
/// `impl Reader` from [`Lexicon::freeze`](crate::Lexicon::freeze).
pub(crate) struct FlatReader {
    offsets: Box<[u32]>,
    buffer: Box<[u8]>,
}

impl FlatReader {
    pub(crate) fn new(offsets: Box<[u32]>, buffer: Box<[u8]>) -> Self {
        Self { offsets, buffer }
    }
}

impl crate::reader::Sealed for FlatReader {}

impl Reader for FlatReader {
    #[inline]
    fn try_resolve(&self, sym: Sym) -> Option<&str> {
        crate::storage::resolve(&self.offsets, &self.buffer, sym.dense())
    }

    #[inline]
    fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    fn iter(&self) -> Box<dyn Iterator<Item = (Sym, &str)> + '_> {
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        Box::new((0..offsets.len() - 1).map(move |i| (Sym::pack_dense(i), crate::storage::str_at(offsets, buffer, i))))
    }
}
