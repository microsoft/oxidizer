// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`LocalReader`]: all strings in one contiguous, dense-indexed layout.
//!
//! Resolves handles through the crate's [`storage`](crate::storage) helpers, which
//! own the (single) unchecked-UTF-8 conversion — this module itself is
//! `unsafe`-free.

use alloc::boxed::Box;

use crate::reader::Reader;
use crate::sym::{Sym, dense_index_of, dense_sym_at};

/// A frozen [`LocalLexicon`](crate::LocalLexicon): every string in one contiguous
/// buffer, addressed by a dense 0-based index.
///
/// Returned by [`LocalLexicon::freeze`](crate::LocalLexicon::freeze). Dropping the
/// dedup hash table is where the memory saving comes from, so this reader can
/// resolve handles but cannot look strings up — see
/// [`freeze`](crate::LocalLexicon::freeze) for how to rebuild a lookup index if you
/// need one.
///
/// Handles minted by the source lexicon stay valid, and their dense numbering is
/// preserved, so [`index_of`](Self::index_of) and [`sym_at`](Self::sym_at) agree
/// with the lexicon they came from.
///
/// # Examples
///
/// ```
/// use internity::{LocalLexicon, LocalReader, Reader};
///
/// let mut lexicon = LocalLexicon::new();
/// let hello = lexicon.intern("hello");
///
/// // The concrete type can be named, so it can be stored in a struct by value.
/// let reader: LocalReader = lexicon.freeze();
/// assert_eq!(reader.resolve(hello), "hello");
/// ```
#[derive(Clone)]
pub struct LocalReader {
    offsets: Box<[u32]>,
    buffer: Box<[u8]>,
}

impl LocalReader {
    pub(crate) fn new(offsets: Box<[u32]>, buffer: Box<[u8]>) -> Self {
        Self { offsets, buffer }
    }

    /// Returns the 0-based position of `sym`, or `None` if it is out of range for
    /// this reader.
    ///
    /// See [`LocalLexicon::index_of`](crate::LocalLexicon::index_of) for what the
    /// index means and what it is useful for.
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let a = lexicon.intern("a");
    /// let reader = lexicon.freeze();
    /// assert_eq!(reader.index_of(a), Some(0));
    /// ```
    #[inline]
    #[must_use]
    pub fn index_of(&self, sym: Sym) -> Option<usize> {
        dense_index_of(self.len(), sym)
    }

    /// Returns the handle at 0-based position `index`, or `None` if this reader
    /// holds fewer than `index + 1` strings.
    ///
    /// See [`LocalLexicon::sym_at`](crate::LocalLexicon::sym_at).
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let a = lexicon.intern("a");
    /// let reader = lexicon.freeze();
    /// assert_eq!(reader.sym_at(0), Some(a));
    /// assert_eq!(reader.sym_at(1), None);
    /// ```
    #[inline]
    #[must_use]
    pub fn sym_at(&self, index: usize) -> Option<Sym> {
        dense_sym_at(self.len(), index)
    }
}

impl core::fmt::Debug for LocalReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalReader")
            .field("len", &Reader::len(self))
            .finish_non_exhaustive()
    }
}

impl crate::reader::Sealed for LocalReader {}
impl Reader for LocalReader {
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
