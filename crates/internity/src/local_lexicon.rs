// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The single-threaded [`LocalLexicon`]: a fast, flat string interner.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::hash::{BuildHasher, Hasher};

use hashbrown::HashTable;
use rustc_hash::FxBuildHasher;

use crate::local_reader::LocalReader;
use crate::reader::Reader;
use crate::sym::{Sym, dense_index_of, dense_sym_at};

/// A single-threaded string interner.
///
/// Maps each distinct string to a compact 4-byte [`Sym`] handle, and resolves
/// handles back to strings. Interning takes `&mut self`; resolving takes `&self`.
/// Use it when building the table from one thread; to intern concurrently, use
/// [`ThreadedLexicon`](crate::ThreadedLexicon) instead.
///
/// The usual lifecycle is **intern, then freeze, then read**: call
/// [`freeze`](LocalLexicon::freeze) once you're done interning to get a cheap,
/// `Send + Sync` [`LocalReader`] whose lookups are lock-free. Existing [`Sym`]
/// handles stay valid across the freeze, and keep their dense numbering.
///
/// Capacity is bounded by a 4 GiB string buffer.
///
/// # Choosing a hasher
///
/// <div class="warning">
///
/// The default hasher is fast but **not collision-attack resistant**. Interning
/// attacker-controlled strings — names off the wire in an XML, JSON, or other
/// protocol parser — with the default hasher invites hash-collision denial of
/// service. Supply a defensive hasher with
/// [`with_hasher`](LocalLexicon::with_hasher) whenever an attacker can choose the
/// strings.
///
/// This differs from `lasso`, which defaults to a collision-resistant hasher. A
/// type-for-type migration therefore needs a deliberate hasher choice, or it
/// silently loses that protection.
///
/// </div>
///
/// # Memory and resolve cost
///
/// Strings are stored as one contiguous buffer plus a table of `u32` boundaries,
/// so each string costs **4 bytes** of index overhead. An interner built on
/// `Vec<&str>` — `lasso`, for example — stores a pointer-and-length pair per
/// string instead, which is two machine words: 16 bytes on a 64-bit target, 8 on
/// a 32-bit one.
///
/// The trade is on the read side: resolving reconstructs the string from two
/// adjacent boundaries, which is one extra dependent memory load compared with
/// loading a ready-made pointer and length. Expect resolve to cost a few
/// instructions more than a `Vec<&str>` design, in exchange for a quarter of the
/// per-string overhead on a 64-bit target (half on a 32-bit one) and far better
/// locality. For resolve-heavy workloads,
/// [`freeze`](LocalLexicon::freeze) first — a [`LocalReader`] is faster than the
/// live lexicon.
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
    /// interner still grows automatically past it, and the usual limits (less
    /// than 4 GiB of string bytes, approximately 4.29 billion strings) still
    /// apply. `strings` sizes the dedup table and offset index; `bytes` sizes the
    /// string buffer.
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
    /// Panics if the total interned bytes would exceed the `u32` buffer limit.
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
    /// Panics if the total interned bytes would exceed the `u32` buffer limit.
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

    /// Returns the 0-based position of `sym` in insertion order, or `None` if it
    /// is out of range for this lexicon.
    ///
    /// Positions are assigned consecutively from zero in insertion order, so this
    /// index is a *dense* key: per-symbol data can live in a `Vec<T>` indexed by
    /// it, rather than a hash map keyed by the handle. That avoids hashing and
    /// probing entirely, and makes set membership a bitset. See
    /// [`SymMap`](crate::SymMap) for the hash map alternative when a dense table
    /// would be too sparse.
    ///
    /// The raw handle value from [`Sym::as_u32`] is 1-based, so it is *not* a
    /// side-table index; `index_of` and [`sym_at`](Self::sym_at) are the supported
    /// conversions.
    ///
    /// This numbering is a guarantee, not an implementation detail, and
    /// [`freeze`](Self::freeze) preserves it — see
    /// [`LocalReader::index_of`](crate::LocalReader::index_of).
    ///
    /// The range check is against the *current* length: a handle from a different
    /// `LocalLexicon` that happens to be in range returns an index here, and a
    /// side table indexed with it would silently read the wrong row. This is not a
    /// memory-safety problem, but it is silent, so keep side tables with the
    /// lexicon they describe.
    ///
    /// `ThreadedLexicon` handles are **not** dense — they encode a shard index in
    /// their high bits — so it deliberately offers no equivalent.
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let a = lexicon.intern("a");
    /// let b = lexicon.intern("b");
    ///
    /// assert_eq!(lexicon.index_of(a), Some(0));
    /// assert_eq!(lexicon.index_of(b), Some(1));
    ///
    /// // A dense side table: one slot per symbol, no hashing.
    /// let mut lengths = vec![0usize; lexicon.len()];
    /// for (sym, s) in lexicon.iter() {
    ///     let i = lexicon
    ///         .index_of(sym)
    ///         .expect("handle came from this lexicon");
    ///     lengths[i] = s.len();
    /// }
    /// assert_eq!(lengths, vec![1, 1]);
    /// ```
    #[inline]
    #[must_use]
    pub fn index_of(&self, sym: Sym) -> Option<usize> {
        dense_index_of(self.len(), sym)
    }

    /// Returns the handle at 0-based position `index`, or `None` if fewer than
    /// `index + 1` strings have been interned.
    ///
    /// The inverse of [`index_of`](Self::index_of); see it for what the index
    /// means.
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::LocalLexicon;
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let a = lexicon.intern("a");
    ///
    /// assert_eq!(lexicon.sym_at(0), Some(a));
    /// assert_eq!(lexicon.sym_at(1), None);
    /// ```
    #[inline]
    #[must_use]
    pub fn sym_at(&self, index: usize) -> Option<Sym> {
        dense_sym_at(self.len(), index)
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

    /// Freezes into a `Send + Sync` [`LocalReader`]. Handles remain valid.
    ///
    /// This trades interning for a smaller, read-optimized table: the dedup hash
    /// map is dropped and the storage is shrunk to fit, which is where the memory
    /// saving comes from. The consequence is that a frozen reader can
    /// [`resolve`](Reader::resolve) handles but has no
    /// [`get`](LocalLexicon::get) — looking a string up is exactly the hash map
    /// that was just freed.
    ///
    /// If you need string lookup on a frozen table, keep the lexicon live instead,
    /// or rebuild a smaller index from [`iter`](Reader::iter). A sorted handle
    /// list costs 4 bytes per string against the roughly 7 the hash map costs,
    /// and answers lookups in `O(log n)`:
    ///
    /// ```
    /// use internity::{LocalLexicon, LocalReader, Reader, Sym};
    ///
    /// struct Frozen {
    ///     reader: LocalReader,
    ///     by_string: Box<[Sym]>,
    /// }
    ///
    /// impl Frozen {
    ///     fn new(reader: LocalReader) -> Self {
    ///         let mut by_string: Vec<Sym> = reader.iter().map(|(sym, _)| sym).collect();
    ///         by_string.sort_by_cached_key(|&sym| reader.resolve(sym));
    ///         Self {
    ///             by_string: by_string.into_boxed_slice(),
    ///             reader,
    ///         }
    ///     }
    ///
    ///     fn get(&self, needle: &str) -> Option<Sym> {
    ///         self.by_string
    ///             .binary_search_by(|&sym| self.reader.resolve(sym).cmp(needle))
    ///             .ok()
    ///             .map(|i| self.by_string[i])
    ///     }
    /// }
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let hello = lexicon.intern("hello");
    /// let frozen = Frozen::new(lexicon.freeze());
    ///
    /// assert_eq!(frozen.get("hello"), Some(hello));
    /// assert_eq!(frozen.get("absent"), None);
    /// ```
    #[must_use]
    pub fn freeze(self) -> LocalReader {
        self.into_reader()
    }

    fn into_reader(self) -> LocalReader {
        LocalReader::new(self.offsets.into_boxed_slice(), self.buffer.into_bytes().into_boxed_slice())
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
