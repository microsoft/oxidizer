// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Shard`]: `RwLock`-guarded interning state.
//!
//! A shard is a [`ShardWrite`] behind a `RwLock`. Because a
//! [`ThreadedLexicon`](crate::ThreadedLexicon) is fill-then-freeze — interning
//! happens under the lock and resolution only happens after
//! [`freeze`](crate::ThreadedLexicon::freeze) — the shard needs no lock-free
//! reader machinery: plain `Vec`s under the lock suffice, and the whole module is
//! free of `unsafe`.
//!
//! Interning takes an upgradable read guard. A miss atomically upgrades it to
//! exclusive access without a second dedup probe; ordinary lookups can still
//! acquire shared read guards while an interning hit holds the upgradable guard.

use core::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::{RwLock, RwLockReadGuard, RwLockUpgradableReadGuard};

use crate::shard_reader::ShardReader;
use crate::shard_write::ShardWrite;
use crate::sym::Sym;

/// A held read guard on a shard's interning state.
///
/// Exposed so a caller can hold guards on *every* shard simultaneously and take a
/// point-in-time snapshot of the whole interner (see
/// [`ThreadedLexicon::freeze`](crate::threaded_lexicon::ThreadedLexicon::freeze)).
pub(crate) type ShardReadGuard<'a> = RwLockReadGuard<'a, ShardWrite>;

/// One shard: interning state guarded by a `RwLock`.
///
/// `#[repr(align(128))]` places each shard lock in a separate 128-byte-aligned
/// region, reducing false sharing on architectures with cache lines up to that
/// size.
#[repr(align(128))]
pub(crate) struct Shard {
    state: RwLock<ShardWrite>,
    // A shard cannot exceed LOCAL_MASK entries, so this cannot wrap even on
    // 32-bit targets.
    generation: AtomicUsize,
}

impl Shard {
    pub(crate) fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self {
            state: RwLock::new(ShardWrite::with_capacity(strings, bytes)),
            generation: AtomicUsize::new(0),
        }
    }

    /// Number of strings interned in this shard.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.state.read().len()
    }

    /// Interns `s` (hash `h`) into the shard identified by `idx`.
    ///
    /// An upgradable guard serializes same-shard interners but lets `get` take
    /// shared read guards. A miss upgrades atomically, so it needs no recheck.
    #[inline]
    pub(crate) fn intern(&self, idx: usize, h: u64, s: &str) -> Sym {
        let up = self.state.upgradable_read();
        if let Some(sym) = up.get(h, s) {
            return sym;
        }
        let mut w = RwLockUpgradableReadGuard::upgrade(up);
        let sym = w.insert_new(idx, h, s);
        self.generation.fetch_add(1, Ordering::Release);
        sym
    }

    /// Interns the UTF-8 string held in `bytes` (hash `h`), validating UTF-8 only
    /// on a dedup miss.
    ///
    /// Mirrors [`intern`](Self::intern), but probes by raw bytes so an existing
    /// entry returns without re-validating: it was validated on first insertion.
    /// A miss validates before upgrading to the write lock.
    #[inline]
    pub(crate) fn intern_bytes(&self, idx: usize, h: u64, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error> {
        let up = self.state.upgradable_read();
        if let Some(sym) = up.get_bytes(h, bytes) {
            return Ok(sym);
        }
        let s = core::str::from_utf8(bytes)?;
        let mut w = RwLockUpgradableReadGuard::upgrade(up);
        let sym = w.insert_new(idx, h, s);
        self.generation.fetch_add(1, Ordering::Release);
        Ok(sym)
    }

    pub(crate) fn generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    /// Looks up `s` without interning it.
    #[inline]
    pub(crate) fn get(&self, h: u64, s: &str) -> Option<Sym> {
        self.state.read().get(h, s)
    }

    /// Freezes this shard into a flat [`ShardReader`], moving its
    /// `(offsets, bytes)` blob out with no copy or re-walk.
    pub(crate) fn freeze(self) -> ShardReader {
        let (offsets, bytes) = self.state.into_inner().into_parts();
        ShardReader::new(offsets.into_boxed_slice(), bytes.into_boxed_slice())
    }

    /// Acquires a read guard on this shard's interning state.
    ///
    /// Callers hold a guard on every shard at once to establish a point-in-time
    /// snapshot boundary: while any read guard is held, an `intern` miss
    /// cannot acquire the exclusive write lock, so no insertion can commit.
    pub(crate) fn read_guard(&self) -> ShardReadGuard<'_> {
        self.state.read()
    }

    #[cfg(test)]
    pub(crate) fn write_available(&self) -> bool {
        self.state.try_write().is_some()
    }

    /// Copies the `(offsets, bytes)` blob into a flat [`ShardReader`] from an
    /// already-held read guard. Used to freeze while the interner is still shared:
    /// the caller holds guards on all shards first, so the resulting reader is a
    /// point-in-time snapshot rather than a per-shard-torn one.
    pub(crate) fn snapshot_locked(guard: &ShardReadGuard<'_>) -> ShardReader {
        let (offsets, bytes) = guard.parts();
        ShardReader::new(offsets.to_vec().into_boxed_slice(), bytes.to_vec().into_boxed_slice())
    }

    #[cfg(test)]
    pub(crate) fn capacities(&self) -> (usize, usize, usize) {
        self.state.read().capacities()
    }
}
