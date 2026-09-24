// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ThreadedLexicon`]: a cheap, cloneable `Arc` handle to a concurrent
//! interner, plus its shared inner state `ThreadedLexiconInner`.

use alloc::boxed::Box;
use alloc::sync::{Arc, Weak};
use core::hash::{BuildHasher, Hasher};

use parking_lot::Mutex;
use rustc_hash::FxBuildHasher;

use crate::reader::Reader;
use crate::shard::{Shard, ShardReadGuard};
use crate::shard_reader::ShardReader;
use crate::sym::{NUM_SHARDS, SHARD_BITS, Sym};
use crate::threaded_reader::ThreadedReader;

/// Mixing constant (golden-ratio) used to decorrelate the shard selector from the
/// bits `hashbrown` consumes for its control byte and bucket index.
const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// Below this average, eager allocation in all shards costs more than their
/// likely growth.
///
/// With 64 shards, a floor of 8 means preallocation from a capacity hint only
/// engages once the hint exceeds approximately 512 strings; below that, eagerly
/// sizing every shard wastes memory on the many shards that stay empty for a
/// small corpus.
const MIN_PREALLOCATED_STRINGS_PER_SHARD: usize = 8;

type SnapshotGenerations = [usize; NUM_SHARDS];
type CachedSnapshot = (SnapshotGenerations, Weak<[ShardReader; NUM_SHARDS]>);

/// A concurrent string interner.
///
/// Maps each distinct string to a compact 4-byte [`Sym`] handle. Interning takes
/// `&self`, so many threads can intern at once. The [`ThreadedLexicon`] itself is
/// a cheap `Clone` (it's `Arc`-backed), so you can hand clones to threads without
/// wrapping it in an `Arc` yourself.
///
/// It is **fill-then-freeze**: intern from as many threads as you like, then call
/// [`freeze`](Self::freeze) to get a `Send + Sync` [`ThreadedReader`] for the read
/// phase, whose lookups are lock-free. Handles stay valid across the freeze.
///
/// Interning is optimized for concurrent fill across shards. Same-shard
/// interners take an upgradable read guard, while ordinary `get` readers can
/// still run concurrently; misses atomically upgrade to a write guard.
/// Freeze before a read-heavy phase rather than using repeated `intern`
/// calls as lookups.
///
/// Handles encode a shard index alongside a per-shard position, so unlike
/// [`LocalLexicon`](crate::LocalLexicon) they are **not** numbered consecutively.
/// Do not use their numeric value as an index into a side table.
///
/// # Choosing a hasher
///
/// <div class="warning">
///
/// The default hasher is fast but **not collision-attack resistant**. Interning
/// attacker-controlled strings — names off the wire in an XML, JSON, or other
/// protocol parser — with the default hasher invites hash-collision denial of
/// service. Supply a defensive hasher with
/// [`with_hasher`](ThreadedLexicon::with_hasher) whenever an attacker can choose
/// the strings.
///
/// This differs from `lasso`, which defaults to a collision-resistant hasher. A
/// type-for-type migration therefore needs a deliberate hasher choice, or it
/// silently loses that protection.
///
/// </div>
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
        Self(Arc::new(ThreadedLexiconInner::with_default_hasher(0, 0)))
    }

    /// Creates an interner preallocated for `strings` strings and `bytes` bytes.
    ///
    /// Uses the default hasher ([`FxBuildHasher`]) and spreads the capacity
    /// approximately evenly across shards. This is a capacity hint: uneven hash
    /// distribution can make one shard grow before the total number or bytes of
    /// strings reaches the requested capacity.
    #[must_use]
    pub fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self(Arc::new(ThreadedLexiconInner::with_default_hasher(strings, bytes)))
    }

    pub(crate) fn with_capacity_for_size_hint(strings: usize) -> Self {
        if strings / NUM_SHARDS < MIN_PREALLOCATED_STRINGS_PER_SHARD {
            Self::new()
        } else {
            Self::with_capacity(strings, 0)
        }
    }
}

impl<S: BuildHasher> ThreadedLexicon<S> {
    /// Creates an empty interner using the given hasher.
    ///
    /// On 32-bit targets, the default constructors emulate the 64-bit
    /// widening-multiply Fx variant to preserve handles from 64-bit targets
    /// using that variant. Passing `FxBuildHasher` explicitly here instead
    /// uses its native 32-bit output and does not preserve those handles.
    pub fn with_hasher(hasher: S) -> Self {
        Self(Arc::new(ThreadedLexiconInner::with_hasher(hasher)))
    }

    /// Like [`with_capacity`](ThreadedLexicon::with_capacity) but with the given hasher.
    ///
    /// See [`with_capacity`](ThreadedLexicon::with_capacity) for the meaning of
    /// the capacity arguments. On 32-bit targets, passing `FxBuildHasher`
    /// explicitly uses its native output rather than the portable default.
    pub fn with_capacity_and_hasher(strings: usize, bytes: usize, hasher: S) -> Self {
        Self(Arc::new(ThreadedLexiconInner::with_capacity_and_hasher(strings, bytes, hasher)))
    }

    /// Interns `s`, returning its handle. Equal strings return equal handles.
    ///
    /// # Panics
    ///
    /// Panics if the owning shard would exceed its capacity (approximately 67
    /// million distinct strings) or the `u32` byte-offset limit.
    #[inline]
    pub fn intern(&self, s: impl AsRef<str>) -> Sym {
        self.0.intern(s.as_ref())
    }

    /// Interns the UTF-8 string in `bytes`, returning its handle.
    ///
    /// Validates UTF-8 at most once per distinct *valid* byte sequence.
    /// Interning takes `&self`, so many threads can call this concurrently, just
    /// like [`intern`](Self::intern).
    ///
    /// Unlike [`intern`](Self::intern), whose `&str` input the caller has already
    /// validated, this accepts raw bytes and runs the `str::from_utf8` check
    /// itself — but *only on a dedup miss*. On a hit, the stored entry is byte-equal
    /// to `bytes` and was validated when first inserted, so it is already known to
    /// be valid UTF-8 and the check is skipped.
    ///
    /// This amortizes UTF-8 validation across duplicate inserts. Interning targets
    /// high-duplication workloads, so feeding raw `&[u8]` straight from a parser or
    /// I/O buffer validates each distinct valid string once — on its first insert —
    /// rather than on every occurrence, while the frozen [`Reader`] still hands
    /// back a checked `&str`.
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
    /// Panics if the owning shard would exceed its capacity (approximately 67
    /// million distinct strings) or the `u32` byte-offset limit.
    ///
    /// # Examples
    ///
    /// ```
    /// use internity::{Reader, ThreadedLexicon};
    ///
    /// # fn main() -> Result<(), core::str::Utf8Error> {
    /// let lexicon = ThreadedLexicon::new();
    /// let a = lexicon.intern_bytes(b"hello")?;
    /// assert!(lexicon.intern_bytes(&[0xff, 0xfe]).is_err()); // invalid UTF-8
    /// assert_eq!(lexicon.freeze().resolve(a), "hello");
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn intern_bytes(&self, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error> {
        self.0.intern_bytes(bytes)
    }

    /// Returns the handle for `s` if already interned, without interning it.
    #[inline]
    #[must_use]
    pub fn get(&self, s: impl AsRef<str>) -> Option<Sym> {
        self.0.get(s.as_ref())
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

    /// Freezes the lexicon into a read-only [`Reader`]; existing handles stay valid.
    ///
    /// Trades interning for lock-free, atomic-free resolution. If this is the only
    /// handle to the interner, each shard's `(offsets, bytes)`
    /// blob is *moved* into the reader (no copy). If other clones are still alive,
    /// a retained, unchanged snapshot can be reused; otherwise the blobs are
    /// copied.
    ///
    /// The result is a point-in-time snapshot even when other clones are still
    /// interning: a cached snapshot represents an instant before any later
    /// insertion, and the copy path holds a read guard on every shard before
    /// copying any of them. Every completed insertion observed up to that instant
    /// is present; insertions that commit afterwards are not. `freeze` leaves
    /// the interner usable, so other clones keep interning and may themselves
    /// freeze independently.
    #[must_use]
    pub fn freeze(self) -> ThreadedReader {
        self.into_reader()
    }

    fn into_reader(self) -> ThreadedReader {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => inner.into_reader(),
            Err(arc) => arc.build_reader(),
        }
    }

    pub(crate) fn into_boxed_reader(self) -> Box<dyn Reader> {
        Box::new(self.into_reader())
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
        let iter = iter.into_iter();
        let (strings, _) = iter.size_hint();
        let lexicon = Self::with_capacity_for_size_hint(strings);
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
/// Interning takes `&self` (each shard is independently lock-guarded), so this is
/// `Sync`.
struct ThreadedLexiconInner<S = FxBuildHasher> {
    shards: [Shard; NUM_SHARDS],
    hasher: S,
    cached_snapshot: Mutex<Option<CachedSnapshot>>,
    #[cfg(target_pointer_width = "32")]
    stable_default_hash: bool,
}

impl ThreadedLexiconInner {
    fn with_default_hasher(strings: usize, bytes: usize) -> Self {
        let inner = Self::with_capacity_and_hasher(strings, bytes, FxBuildHasher);
        #[cfg(target_pointer_width = "32")]
        let inner = {
            let mut inner = inner;
            inner.stable_default_hash = true;
            inner
        };
        inner
    }
}

impl<S: BuildHasher> ThreadedLexiconInner<S> {
    /// Creates empty inner state using the given hasher.
    fn with_hasher(hasher: S) -> Self {
        Self::with_capacity_and_hasher(0, 0, hasher)
    }

    fn with_capacity_and_hasher(strings: usize, bytes: usize, hasher: S) -> Self {
        let strings_per_shard = strings.div_ceil(NUM_SHARDS);
        let bytes_per_shard = bytes.div_ceil(NUM_SHARDS);
        Self {
            shards: core::array::from_fn(|_| Shard::with_capacity(strings_per_shard, bytes_per_shard)),
            hasher,
            cached_snapshot: Mutex::new(None),
            #[cfg(target_pointer_width = "32")]
            stable_default_hash: false,
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
        #[cfg(target_pointer_width = "32")]
        if self.stable_default_hash {
            return crate::stable_fx::hash_bytes_64(b);
        }
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
        self.shards[idx].intern(idx, h, s)
    }

    /// Interns the UTF-8 string held in `bytes`, validating UTF-8 only on a miss.
    #[inline]
    fn intern_bytes(&self, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error> {
        let h = self.hash_bytes(bytes);
        let idx = Self::shard_of(h);
        self.shards[idx].intern_bytes(idx, h, bytes)
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
    fn into_reader(self) -> ThreadedReader {
        ThreadedReader::new(self.shards.map(Shard::freeze))
    }

    /// Reuses the previous immutable snapshot when no strings were inserted
    /// since its creation; otherwise copies the shards into a new reader.
    ///
    /// Read guards on *all* shards are acquired up front, before any blob is
    /// copied. The instant every guard is held no `intern` can commit (a miss
    /// cannot acquire the write lock while a reader is present), so the copied
    /// state is a single point-in-time snapshot rather than a per-shard-torn one.
    /// Guards are taken in index order and `intern` only ever locks one shard, so
    /// this cannot deadlock. Concurrent lookups keep running; only in-flight
    /// insertions briefly stall for the copy. The cache lock remains held while
    /// a snapshot is copied so simultaneous misses share one result instead of
    /// copying every shard independently.
    fn build_reader(&self) -> ThreadedReader {
        self.build_reader_with_hook(|| {})
    }

    fn build_reader_with_hook(&self, after_first_copy: impl FnOnce()) -> ThreadedReader {
        let mut cache = self.cached_snapshot.lock();
        let generations = core::array::from_fn(|i| self.shards[i].generation());
        if let Some((cached_generations, shards)) = cache.as_ref()
            && *cached_generations == generations
            && let Some(shards) = shards.upgrade()
        {
            return ThreadedReader::from_shared(shards);
        }

        let guards: [ShardReadGuard<'_>; NUM_SHARDS] = core::array::from_fn(|i| self.shards[i].read_guard());
        let generations = core::array::from_fn(|i| self.shards[i].generation());
        let mut hook = Some(after_first_copy);
        let readers = core::array::from_fn(|i| {
            let reader = Shard::snapshot_locked(&guards[i]);
            if i == 0 {
                hook.take().expect("callback runs once after the first shard")();
            }
            reader
        });
        drop(guards);
        let reader = ThreadedReader::new(readers);
        *cache = Some((generations, reader.downgrade()));
        reader
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::{NUM_SHARDS, ThreadedLexicon};
    use crate::Reader;

    #[test]
    fn shared_snapshot_cache_reuses_unchanged_data_and_invalidates_on_insert() {
        let lexicon = ThreadedLexicon::new();
        let first_handle = lexicon.intern("first");
        let first = lexicon.clone().freeze();
        let second = lexicon.clone().freeze();
        assert!(first.shares_storage_with(&second));
        assert_eq!(first.resolve(first_handle), "first");
        assert_eq!(lexicon.intern("first"), first_handle);
        assert!(first.shares_storage_with(&lexicon.clone().freeze()));

        let writer = lexicon.clone();
        let inserted = std::thread::spawn(move || writer.intern_bytes(b"new").unwrap()).join().unwrap();
        let third = lexicon.clone().freeze();
        assert!(!first.shares_storage_with(&third));
        assert_eq!(first.try_resolve(inserted), None);
        assert_eq!(third.resolve(inserted), "new");
        assert!(third.shares_storage_with(&lexicon.clone().freeze()));

        let clone = third.clone();
        assert!(clone.shares_storage_with(&third));
        assert_eq!(clone.resolve(inserted), "new");
        drop(clone);
        drop(third);
        assert!(lexicon.0.cached_snapshot.lock().as_ref().unwrap().1.upgrade().is_none());
        assert_eq!(lexicon.freeze().resolve(inserted), "new");
    }

    #[test]
    fn shared_freeze_holds_every_shard_during_copy() {
        let first_word = "key-127";
        let second_word = "key-51";
        let lexicon = ThreadedLexicon::new();

        let snapshot = lexicon.0.build_reader_with_hook(|| {
            for shard in &lexicon.0.shards {
                assert!(!shard.write_available(), "every shard must remain read-locked throughout the copy");
            }
        });
        let first = lexicon.intern(first_word);
        let second = lexicon.intern(second_word);
        assert_eq!(snapshot.try_resolve(first), None);
        assert_eq!(snapshot.try_resolve(second), None);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "preallocation capacity is a safe allocation policy covered by native tests; Miri retains threaded interning and resolution"
    )]
    fn from_iter_preallocates_from_large_lower_size_hint() {
        let strings = NUM_SHARDS * 8;
        let lexicon: ThreadedLexicon = (0..strings).map(|_| "same").collect();

        for shard in &lexicon.0.shards {
            let (dedup, offsets, _) = shard.capacities();
            assert!(dedup >= 8);
            assert!(offsets >= 9);
        }
    }

    #[test]
    fn from_iter_does_not_preallocate_every_shard_for_small_hint() {
        let lexicon: ThreadedLexicon = core::iter::once("same").collect();
        let allocated_shards = lexicon.0.shards.iter().filter(|shard| shard.capacities().0 != 0).count();

        assert_eq!(allocated_shards, 1);
    }
}
