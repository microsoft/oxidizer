// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The single-threaded [`Lexicon`]: a fast, flat string interner.

use alloc::string::String;
use alloc::vec::Vec;
use core::hash::{BuildHasher, Hasher};

use hashbrown::HashTable;
use rustc_hash::FxBuildHasher;

use crate::flat_reader::FlatReader;
use crate::reader::Reader;
use crate::sym::Sym;

/// A fast, single-threaded string interner.
///
/// Maps each distinct string to a compact 4-byte [`Sym`] handle, and resolves
/// handles back to strings. Interning takes `&mut self`; resolving takes `&self`.
/// This is the fastest choice when you build the table from a single thread — to
/// intern concurrently from many threads, use [`ThreadedLexicon`](crate::ThreadedLexicon) instead.
///
/// The usual lifecycle is **intern, then freeze, then read**: call
/// [`freeze`](Lexicon::freeze) once you're done interning to get a cheap,
/// `Send + Sync` [`Reader`] whose lookups are lock-free. Existing [`Sym`] handles
/// stay valid across the freeze.
///
/// By default, [`Lexicon`] uses a fast, non-cryptographic hasher; supply your own with
/// [`with_hasher`](Lexicon::with_hasher) (for example a DoS-resistant one when
/// interning untrusted input). Capacity is bounded by a 4 GB string buffer.
///
/// # Examples
///
/// ```
/// use internity::Lexicon;
///
/// let mut lexicon = Lexicon::new();
/// let a = lexicon.intern("hello");
/// assert_eq!(lexicon.intern("hello"), a); // dedup: same string → same handle
/// assert_ne!(lexicon.intern("world"), a);
/// assert_eq!(lexicon.resolve(a), "hello");
/// ```
pub struct Lexicon<S = FxBuildHasher> {
    dedup: HashTable<Sym>,
    /// String boundaries into `buffer`, CSR-style: `offsets[i]` is the start and
    /// `offsets[i+1]` the end of the `i`-th string. Always starts with a `0`
    /// sentinel, so it holds `len() + 1` entries. This lets resolve read `start`
    /// and `end` from two adjacent slots — no per-index branch.
    offsets: Vec<u32>,
    /// All interned strings concatenated.
    buffer: String,
    hasher: S,
}

impl Default for Lexicon {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> core::fmt::Debug for Lexicon<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Lexicon")
            .field("len", &(self.offsets.len() - 1))
            .finish_non_exhaustive()
    }
}

impl Lexicon {
    /// Creates an empty interner with the default hasher ([`FxBuildHasher`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_hasher(FxBuildHasher)
    }

    /// Creates an interner with the default hasher ([`FxBuildHasher`]),
    /// preallocated to hold `strings` distinct strings totaling `bytes` bytes
    /// without reallocating.
    ///
    /// This is a capacity *hint*: the interner still grows automatically past it,
    /// and the usual limits (≤ 4 GB of string bytes, approximately 4.29 billion
    /// strings) still apply. `strings` sizes the dedup table and offset index;
    /// `bytes` sizes the string buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::Lexicon;
    ///
    /// // Expecting ~1000 strings averaging 16 bytes.
    /// let mut lexicon = Lexicon::with_capacity(1000, 16 * 1000);
    /// let a = lexicon.intern("hello");
    /// assert_eq!(lexicon.resolve(a), "hello");
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Capacity hints affect allocation behavior, not observable values.
    pub fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self::with_capacity_and_hasher(strings, bytes, FxBuildHasher)
    }
}

impl<S: BuildHasher> Lexicon<S> {
    /// Creates an empty interner using the given hasher.
    pub fn with_hasher(hasher: S) -> Self {
        Self::with_capacity_and_hasher(0, 0, hasher)
    }

    /// Creates an interner using the given hasher, preallocated to hold `strings`
    /// distinct strings totaling `bytes` bytes without reallocating.
    ///
    /// See [`with_capacity`](Lexicon::with_capacity) for the meaning of the
    /// capacity arguments.
    pub fn with_capacity_and_hasher(strings: usize, bytes: usize, hasher: S) -> Self {
        let mut offsets = Vec::with_capacity(strings.saturating_add(1));
        offsets.push(0);
        Self {
            dedup: HashTable::with_capacity(strings),
            offsets,
            buffer: String::with_capacity(bytes),
            hasher,
        }
    }

    /// Hashes a string's bytes directly (see the
    /// [`ThreadedLexicon`](crate::ThreadedLexicon) note).
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Constant hashes remain correct under collision resolution, but are pathologically slow.
    fn hash_str(&self, s: &str) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        hasher.write(s.as_bytes());
        hasher.finish()
    }

    /// The string for a dense id, assuming it is in range.
    #[inline]
    fn str_at<'a>(offsets: &[u32], buffer: &'a str, index: usize) -> &'a str {
        crate::storage::str_at(offsets, buffer.as_bytes(), index)
    }

    /// Interns `s`, returning its handle. Equal strings return equal handles.
    ///
    /// # Panics
    ///
    /// Panics if the total interned bytes would exceed 4 GB (the `u32` buffer
    /// limit).
    #[inline]
    pub fn intern(&mut self, s: impl AsRef<str>) -> Sym {
        let s = s.as_ref();
        let h = self.hash_str(s);

        // Fast path (the common case in steady state): a single probe returns an
        // existing handle. Only a genuine miss pays the second (insert) probe.
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        if let Some(&sym) = self.dedup.find(h, |&sym| Self::str_at(offsets, buffer, sym.dense()) == s) {
            return sym;
        }

        let index = self.offsets.len() - 1;
        let end = self
            .buffer
            .len()
            .checked_add(s.len())
            .and_then(|n| u32::try_from(n).ok())
            .expect("internity: buffer exceeds u32");
        self.buffer.push_str(s);
        self.offsets.push(end);
        let sym = Sym::pack_dense(index);

        let offsets = &self.offsets;
        let buffer = &self.buffer;
        let hasher = &self.hasher;
        self.dedup.insert_unique(h, sym, |&sym| {
            let s = Self::str_at(offsets, buffer, sym.dense());
            let mut hh = hasher.build_hasher();
            hh.write(s.as_bytes());
            hh.finish()
        });
        sym
    }

    /// Returns the handle for `s` if already interned, without interning it.
    #[inline]
    pub fn get(&self, s: &str) -> Option<Sym> {
        let h = self.hash_str(s);
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        self.dedup.find(h, |&sym| Self::str_at(offsets, buffer, sym.dense()) == s).copied()
    }

    /// Resolves a handle to its string.
    ///
    /// # Panics
    ///
    /// Panics if `sym` is out of range for this interner. Use
    /// [`Lexicon::try_resolve`] for a non-panicking check.
    #[inline]
    #[must_use]
    pub fn resolve(&self, sym: Sym) -> &str {
        self.try_resolve(sym).expect("internity: Sym does not belong to this interner")
    }

    /// Resolves a handle to its string, or `None` if out of range.
    #[inline]
    #[must_use]
    pub fn try_resolve(&self, sym: Sym) -> Option<&str> {
        let index = sym.dense();
        crate::storage::resolve(&self.offsets, self.buffer.as_bytes(), index)
    }

    /// Number of distinct interned strings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    /// Returns `true` if nothing has been interned yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.offsets.len() == 1
    }

    /// Returns an iterator over `(Sym, &str)` for every interned string, in
    /// handle order.
    ///
    /// ```
    /// use internity::Lexicon;
    ///
    /// let mut lexicon = Lexicon::new();
    /// let a = lexicon.intern("a");
    /// let b = lexicon.intern("b");
    /// let pairs: Vec<_> = lexicon.iter().collect();
    /// assert_eq!(pairs, vec![(a, "a"), (b, "b")]);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (Sym, &str)> + '_ {
        let offsets = &self.offsets;
        let bytes = self.buffer.as_bytes();
        (0..offsets.len() - 1).map(move |i| (Sym::pack_dense(i), crate::storage::str_at(offsets, bytes, i)))
    }

    /// Freezes into a `Send + Sync` [`Reader`]. Handles remain valid.
    #[must_use]
    pub fn freeze(self) -> impl Reader {
        FlatReader::new(self.offsets.into_boxed_slice(), self.buffer.into_bytes().into_boxed_slice())
    }
}

impl<S: BuildHasher, T: AsRef<str>> Extend<T> for Lexicon<S> {
    /// Interns each string from the iterator (discarding the handles).
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for s in iter {
            self.intern(s.as_ref());
        }
    }
}

impl<T: AsRef<str>> FromIterator<T> for Lexicon {
    /// Builds a `Lexicon` (default hasher) by interning every string.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut lexicon = Self::new();
        lexicon.extend(iter);
        lexicon
    }
}
