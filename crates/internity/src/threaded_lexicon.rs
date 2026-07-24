// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ThreadedLexicon`]: a cheap, cloneable `Arc` handle to a concurrent
//! interner, plus its shared inner state `ThreadedLexiconInner`.

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::hash::{BuildHasher, Hasher};

use rustc_hash::FxBuildHasher;

use crate::reader::Reader;
use crate::shard::Shard;
use crate::sharded_reader::ShardedReader;
use crate::sym::{NUM_SHARDS, SHARD_BITS, Sym};

/// Mixing constant (golden-ratio) used to decorrelate the shard selector from the
/// bits `hashbrown` consumes for its control byte and bucket index.
const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// A fast, concurrent string interner.
///
/// Maps each distinct string to a compact 4-byte [`Sym`] handle. Interning takes
/// `&self`, so many threads can intern at once; the handle is a cheap `Clone`
/// (it's `Arc`-backed), so you hand copies to threads directly without wrapping it
/// in an `Arc` yourself.
///
/// It is **fill-then-freeze**: intern from as many threads as you like, then call
/// [`freeze`](Self::freeze) to get a `Send + Sync` [`Reader`] for the read phase,
/// whose lookups are lock-free. Handles stay valid across the freeze.
///
/// By default, [`ThreadedLexicon`] uses a fast, non-cryptographic hasher; supply your own with
/// [`with_hasher`](ThreadedLexicon::with_hasher) (for example a DoS-resistant one
/// when interning untrusted input).
///
/// # Examples
///
/// ```
/// use internity::{Reader, ThreadedLexicon};
///
/// let lexicon = ThreadedLexicon::new();
/// let a = lexicon.intern("hello");
/// let b = lexicon.clone().intern("hello"); // another handle to the same interner
/// assert_eq!(a, b); // identical strings share a handle
/// assert_ne!(a, lexicon.intern("world"));
///
/// let reader = lexicon.freeze(); // read phase
/// assert_eq!(reader.resolve(a), "hello");
/// ```
pub struct ThreadedLexicon<S = FxBuildHasher>(Arc<ThreadedLexiconInner<S>>);

impl<S> Clone for ThreadedLexicon<S> {
    /// Clones the handle (an `Arc` bump), not the interner.
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl Default for ThreadedLexicon {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: BuildHasher> core::fmt::Debug for ThreadedLexicon<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThreadedLexicon")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl ThreadedLexicon {
    /// Creates an empty interner with the default hasher ([`FxBuildHasher`]).
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(ThreadedLexiconInner::new()))
    }
}

impl<S: BuildHasher> ThreadedLexicon<S> {
    /// Creates an empty interner using the given hasher.
    pub fn with_hasher(hasher: S) -> Self {
        Self(Arc::new(ThreadedLexiconInner::with_hasher(hasher)))
    }

    /// Interns `s`, returning its handle. Equal strings always return equal
    /// handles.
    ///
    /// # Panics
    ///
    /// Panics if the owning shard would exceed its capacity (approximately 67
    /// million distinct strings) or 4 GB of bytes.
    #[inline]
    pub fn intern(&self, s: impl AsRef<str>) -> Sym {
        self.0.intern(s.as_ref())
    }

    /// Returns the handle for `s` if it has already been interned, without
    /// interning it.
    #[inline]
    #[must_use]
    pub fn get(&self, s: &str) -> Option<Sym> {
        self.0.get(s)
    }

    /// Returns the number of distinct interned strings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if nothing has been interned yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Freezes the lexicon into a read-only [`Reader`], giving up interning in
    /// exchange for lock-free, atomic-free resolution. Existing [`Sym`] handles
    /// remain valid on the returned `Reader`.
    ///
    /// If this is the only handle to the interner, each shard's `(offsets, bytes)`
    /// blob is *moved* into the reader (no copy). If other clones are still alive,
    /// the blobs are copied instead.
    #[must_use]
    pub fn freeze(self) -> impl Reader {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => inner.into_reader(),
            Err(arc) => arc.build_reader(),
        }
    }
}

impl<S: BuildHasher, T: AsRef<str>> Extend<T> for ThreadedLexicon<S> {
    /// Interns each string from the iterator (discarding the handles).
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for s in iter {
            self.intern(s.as_ref());
        }
    }
}

impl<T: AsRef<str>> FromIterator<T> for ThreadedLexicon {
    /// Builds a [`ThreadedLexicon`] (default hasher) by interning every string.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let lexicon = Self::new();
        for s in iter {
            lexicon.intern(s.as_ref());
        }
        lexicon
    }
}

/// The shared state of a [`ThreadedLexicon`].
///
/// Holds the inline shard array and the hasher. Because the `Arc` already puts
/// this whole struct on the heap, the shard array is stored **inline** (no extra
/// `Box`): interning reaches a shard with a single index into the `Arc`'s payload.
/// Interning takes `&self` (each shard is independently mutex-guarded), so this is
/// `Sync`.
struct ThreadedLexiconInner<S = FxBuildHasher> {
    shards: [Shard; NUM_SHARDS],
    hasher: S,
}

impl ThreadedLexiconInner {
    /// Creates empty inner state with the default hasher ([`FxBuildHasher`]).
    fn new() -> Self {
        Self::with_hasher(FxBuildHasher)
    }
}

impl<S: BuildHasher> ThreadedLexiconInner<S> {
    /// Creates empty inner state using the given hasher.
    fn with_hasher(hasher: S) -> Self {
        Self {
            shards: core::array::from_fn(|_| Shard::new()),
            hasher,
        }
    }

    /// Selects the owning shard for a hash. Uses a multiply-mix so the shard bits
    /// are independent of the hash bits `hashbrown` relies on internally.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Shard selection affects distribution and performance, not correctness.
    fn shard_of(h: u64) -> usize {
        (h.wrapping_mul(MIX) >> (64 - SHARD_BITS)) as usize
    }

    /// Hashes a string's bytes directly.
    ///
    /// Unlike `hash_one(s)` (which routes through `<str as Hash>` and appends a
    /// `0xff` terminator byte — an extra hasher round), this hashes only the bytes.
    /// That is sound here because interner keys are whole strings never combined
    /// with other hashed fields, so no prefix-disambiguation framing is needed.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Constant hashes remain correct under collision resolution, but are pathologically slow.
    fn hash_bytes(&self, b: &[u8]) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        hasher.write(b);
        hasher.finish()
    }

    #[inline]
    fn hash_str(&self, s: &str) -> u64 {
        self.hash_bytes(s.as_bytes())
    }

    /// Interns `s`, returning its handle. Equal strings always return equal
    /// handles.
    #[inline]
    fn intern(&self, s: &str) -> Sym {
        let h = self.hash_str(s);
        let idx = Self::shard_of(h);
        self.shards[idx].intern(idx, h, s, &|t: &[u8]| self.hash_bytes(t))
    }

    /// Returns the handle for `s` if it has already been interned, without
    /// interning it.
    #[inline]
    fn get(&self, s: &str) -> Option<Sym> {
        let h = self.hash_str(s);
        self.shards[Self::shard_of(h)].get(h, s)
    }

    /// Returns the number of distinct interned strings.
    fn len(&self) -> usize {
        self.shards.iter().map(Shard::len).sum()
    }

    /// Consumes the inner state, moving each shard's `(offsets, bytes)` blob into a
    /// [`ShardReader`] with no copy or re-walk.
    fn into_reader(self) -> ShardedReader {
        ShardedReader::new(Box::new(self.shards.map(Shard::freeze)))
    }

    /// Builds a [`ShardedReader`] without consuming, copying each shard's
    /// `(offsets, bytes)` blob. Used when the interner is still shared (outstanding
    /// `Arc` clones).
    fn build_reader(&self) -> ShardedReader {
        ShardedReader::new(Box::new(core::array::from_fn(|i| self.shards[i].snapshot())))
    }
}
