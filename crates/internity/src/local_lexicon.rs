// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The single-threaded [`LocalLexicon`]: a fast, flat string interner.

use alloc::boxed::Box;
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
/// [`freeze`](LocalLexicon::freeze) once you're done interning to get a cheap,
/// `Send + Sync` [`Reader`] whose lookups are lock-free. Existing [`Sym`] handles
/// stay valid across the freeze.
///
/// By default, [`LocalLexicon`] uses a fast, non-cryptographic hasher; supply your own with
/// [`with_hasher`](LocalLexicon::with_hasher) (for example a DoS-resistant one when
/// interning untrusted input). Capacity is bounded by a 4 GB string buffer.
///
/// # Examples
///
/// ```
/// use internity::LocalLexicon;
///
/// let mut lexicon = LocalLexicon::new();
/// let a = lexicon.intern("hello");
/// assert_eq!(lexicon.intern("hello"), a); // dedup: same string → same handle
/// assert_ne!(lexicon.intern("world"), a);
/// assert_eq!(lexicon.resolve(a), "hello");
/// ```
pub struct LocalLexicon<S = FxBuildHasher> {
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

struct StorageRollback<'a> {
    offsets: &'a mut Vec<u32>,
    buffer: &'a mut String,
    index: usize,
}

impl Drop for StorageRollback<'_> {
    fn drop(&mut self) {
        self.offsets.truncate(self.index + 1);
        self.buffer.truncate(self.offsets[self.index] as usize);
    }
}

impl Default for LocalLexicon {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> core::fmt::Debug for LocalLexicon<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalLexicon")
            .field("len", &(self.offsets.len() - 1))
            .finish_non_exhaustive()
    }
}

impl LocalLexicon {
    /// Creates an empty interner with the default hasher ([`FxBuildHasher`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_hasher(FxBuildHasher)
    }

    /// Creates an interner preallocated for `strings` strings and `bytes` bytes.
    ///
    /// Uses the default hasher ([`FxBuildHasher`]). This is a capacity *hint*: the
    /// interner still grows automatically past it,
    /// and the usual limits (≤ 4 GB of string bytes, approximately 4.29 billion
    /// strings) still apply. `strings` sizes the dedup table and offset index;
    /// `bytes` sizes the string buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// // Expecting ~1000 strings averaging 16 bytes.
    /// let mut lexicon = LocalLexicon::with_capacity(1000, 16 * 1000);
    /// let a = lexicon.intern("hello");
    /// assert_eq!(lexicon.resolve(a), "hello");
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Capacity hints affect allocation behavior, not observable values.
    pub fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self::with_capacity_and_hasher(strings, bytes, FxBuildHasher)
    }
}

impl<S: BuildHasher> LocalLexicon<S> {
    /// Creates an empty interner using the given hasher.
    pub fn with_hasher(hasher: S) -> Self {
        Self::with_capacity_and_hasher(0, 0, hasher)
    }

    /// Like [`with_capacity`](LocalLexicon::with_capacity) but with the given hasher.
    ///
    /// See [`with_capacity`](LocalLexicon::with_capacity) for the meaning of the
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

    /// Hashes a byte slice directly (see the
    /// [`ThreadedLexicon`](crate::ThreadedLexicon) note).
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Constant hashes remain correct under collision resolution, but are pathologically slow.
    fn hash_bytes(&self, bytes: &[u8]) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        hasher.write(bytes);
        hasher.finish()
    }

    /// Hashes a string's bytes directly (see the
    /// [`ThreadedLexicon`](crate::ThreadedLexicon) note).
    #[inline]
    fn hash_str(&self, s: &str) -> u64 {
        self.hash_bytes(s.as_bytes())
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
        // existing handle. Only a genuine miss pays the insert path.
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        if let Some(&sym) = self.dedup.find(h, |&sym| Self::str_at(offsets, buffer, sym.dense()) == s) {
            return sym;
        }

        self.insert_new(h, s)
    }

    /// Appends `s` (hash `h`) as a **new** string and returns its handle.
    ///
    /// The caller must have already established that `s` is absent (via a probe
    /// with hash `h`), so this skips the dedup re-probe — calling it for a present
    /// string would create a duplicate handle. Panic-safe: rolls storage back if
    /// the table's growth/rehash or either append unwinds.
    #[inline]
    fn insert_new(&mut self, h: u64, s: &str) -> Sym {
        let index = self.offsets.len() - 1;
        let buffer_len = self.buffer.len();
        let end = buffer_len
            .checked_add(s.len())
            .and_then(|n| u32::try_from(n).ok())
            .expect("internity: buffer exceeds u32");

        // HashTable completes any fallible growth and rehashing before placing
        // the new value. Roll storage back if that work or either append panics.
        let storage = StorageRollback {
            offsets: &mut self.offsets,
            buffer: &mut self.buffer,
            index,
        };
        storage.buffer.push_str(s);
        storage.offsets.push(end);
        let sym = Sym::pack_dense(index);

        let offsets = &*storage.offsets;
        let buffer = &*storage.buffer;
        let hasher = &self.hasher;
        self.dedup.insert_unique(h, sym, |&sym| {
            let s = Self::str_at(offsets, buffer, sym.dense());
            let mut hh = hasher.build_hasher();
            hh.write(s.as_bytes());
            hh.finish()
        });
        core::mem::forget(storage);
        sym
    }

    /// Interns the UTF-8 string in `bytes`, returning its handle.
    ///
    /// Validates UTF-8 at most once per distinct *valid* byte sequence. Unlike
    /// [`intern`](Self::intern), whose `&str` input the caller has already
    /// validated, this accepts raw bytes and runs the `str::from_utf8` check
    /// itself — but *only on a dedup miss*. On a hit, the stored entry is byte-equal
    /// to `bytes` and was validated when first inserted, so it is already known to
    /// be valid UTF-8 and the check is skipped.
    ///
    /// This amortizes UTF-8 validation across duplicate inserts. Interning targets
    /// high-duplication workloads, so feeding raw `&[u8]` straight from a parser or
    /// I/O buffer validates each distinct valid string once — on its first insert —
    /// rather than on every occurrence, while [`resolve`](Self::resolve) still
    /// hands back a checked `&str`.
    ///
    /// # Errors
    ///
    /// Returns a [`Utf8Error`](core::str::Utf8Error) if `bytes` is not valid UTF-8.
    /// The check is skipped only for input that byte-matches an already-interned
    /// (hence valid) string; invalid bytes are never stored, so every call with the
    /// same invalid input re-validates and returns `Err` again.
    ///
    /// # Panics
    ///
    /// Panics if the total interned bytes would exceed 4 GB (the `u32` buffer
    /// limit).
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let a = lexicon.intern_bytes(b"hello").unwrap();
    /// assert_eq!(lexicon.resolve(a), "hello");
    /// assert!(lexicon.intern_bytes(&[0xff, 0xfe]).is_err()); // invalid UTF-8
    /// ```
    #[inline]
    pub fn intern_bytes(&mut self, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error> {
        let h = self.hash_bytes(bytes);
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        if let Some(&sym) = self
            .dedup
            .find(h, |&sym| Self::str_at(offsets, buffer, sym.dense()).as_bytes() == bytes)
        {
            return Ok(sym);
        }

        // Miss: validate once, then insert without re-probing (we just probed with
        // hash `h`). This keeps the byte-probe above load-bearing.
        let s = core::str::from_utf8(bytes)?;
        Ok(self.insert_new(h, s))
    }

    /// Returns the handle for `s` if already interned, without interning it.
    #[inline]
    pub fn get(&self, s: impl AsRef<str>) -> Option<Sym> {
        let s = s.as_ref();
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
    /// [`LocalLexicon::try_resolve`] for a non-panicking check.
    #[inline]
    #[must_use]
    pub fn resolve(&self, sym: Sym) -> &str {
        self.try_resolve(sym).expect("internity: Sym is out of range for this interner")
    }

    /// Resolves a handle to its string, or `None` if out of range.
    ///
    /// The range check is a memory-safety bound, not provenance validation: an
    /// in-range handle minted by a different `LocalLexicon` resolves to whichever
    /// string occupies that slot here. Only out-of-range handles return `None`.
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

    /// Returns an iterator over `(Sym, &str)` for every interned string, in handle order.
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// let mut lexicon = LocalLexicon::new();
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
        self.into_reader()
    }

    fn into_reader(self) -> FlatReader {
        FlatReader::new(self.offsets.into_boxed_slice(), self.buffer.into_bytes().into_boxed_slice())
    }

    pub(crate) fn into_boxed_reader(self) -> Box<dyn Reader> {
        Box::new(self.into_reader())
    }
}

impl<S> crate::reader::Sealed for LocalLexicon<S> {}

impl<S: BuildHasher + Send + Sync> Reader for LocalLexicon<S> {
    fn try_resolve(&self, sym: Sym) -> Option<&str> {
        Self::try_resolve(self, sym)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn iter(&self) -> Box<dyn Iterator<Item = (Sym, &str)> + '_> {
        Box::new(Self::iter(self))
    }
}

impl<S: BuildHasher, T: AsRef<str>> Extend<T> for LocalLexicon<S> {
    /// Interns each string from the iterator (discarding the handles).
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for s in iter {
            self.intern(s.as_ref());
        }
    }
}

impl<T: AsRef<str>> FromIterator<T> for LocalLexicon {
    /// Builds a [`LocalLexicon`] (default hasher) by interning every string.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (strings, _) = iter.size_hint();
        let mut lexicon = Self::with_capacity(strings, 0);
        lexicon.extend(iter);
        lexicon
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::LocalLexicon;

    #[test]
    fn from_iter_preallocates_from_lower_size_hint() {
        let lexicon: LocalLexicon = (0..64).map(|_| "same").collect();

        assert!(lexicon.dedup.capacity() >= 64);
        assert!(lexicon.offsets.capacity() >= 65);
    }
}
