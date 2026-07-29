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
//! `intern` uses an **upgradable-read fast path**: an already-interned string (a
//! dedup hit) resolves under an upgradable read lock, then a genuine miss
//! atomically upgrades that guard to the exclusive write lock to insert.
//!
//! Concurrency trade-off: `parking_lot::RwLock` permits only **one** upgradable
//! read guard at a time, so two `intern` calls landing on the *same* shard
//! serialize against each other even when both are dedup hits. Plain `read()`
//! guards (used by `get`) may coexist with the upgradable guard and with each
//! other. `intern` calls on *different* shards proceed independently. Sharding
//! therefore parallelizes interning across shards, not within a single shard.

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
}

impl Shard {
    pub(crate) fn with_capacity(strings: usize, bytes: usize) -> Self {
        Self {
            state: RwLock::new(ShardWrite::with_capacity(strings, bytes)),
        }
    }

    /// Number of strings interned in this shard.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.state.read().len()
    }

    /// Interns `s` (hash `h`) into the shard identified by `idx`.
    ///
    /// The hit check runs under an **upgradable read lock**, which coexists with
    /// other threads' `read()`s (concurrent lookups don't block it). A miss
    /// upgrades that same guard **atomically** to exclusive access — no other
    /// writer can have inserted in between — so two threads racing on the same new
    /// string can never create two handles, and the insert needs no re-probe.
    #[inline]
    pub(crate) fn intern<R: Fn(&[u8]) -> u64>(&self, idx: usize, h: u64, s: &str, rehash: &R) -> Sym {
        let up = self.state.upgradable_read();
        if let Some(sym) = up.get(h, s) {
            return sym;
        }
        // Miss: upgrade atomically to exclusive access (no other writer can have
        // inserted in between — we held the upgradable lock throughout), then
        // insert without re-probing. Readers of this shard briefly block while
        // the exclusive guard is held.
        let mut w = RwLockUpgradableReadGuard::upgrade(up);
        w.insert_new(idx, h, s, rehash)
    }

    /// Interns the UTF-8 string held in `bytes` (hash `h`), validating UTF-8 only
    /// on a dedup miss.
    ///
    /// Mirrors [`intern`](Self::intern), but probes by raw bytes so an
    /// already-interned entry (a hit) returns without re-validating: the stored
    /// string is byte-equal to `bytes` and was validated on its first insert. Only
    /// a genuine miss runs `str::from_utf8` — while still holding the upgradable
    /// read lock, before upgrading to insert.
    #[inline]
    pub(crate) fn intern_bytes<R: Fn(&[u8]) -> u64>(
        &self,
        idx: usize,
        h: u64,
        bytes: &[u8],
        rehash: &R,
    ) -> Result<Sym, core::str::Utf8Error> {
        let up = self.state.upgradable_read();
        if let Some(sym) = up.get_bytes(h, bytes) {
            return Ok(sym);
        }
        let s = core::str::from_utf8(bytes)?;
        let mut w = RwLockUpgradableReadGuard::upgrade(up);
        Ok(w.insert_new(idx, h, s, rehash))
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
    /// snapshot boundary: while any read guard is held, an in-flight `intern` miss
    /// cannot upgrade to the exclusive write lock, so no insertion can commit.
    pub(crate) fn read_guard(&self) -> ShardReadGuard<'_> {
        self.state.read()
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
