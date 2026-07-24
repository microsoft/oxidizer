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
//! dedup hit) resolves under an upgradable read lock, which coexists with other
//! threads' `read()`s so concurrent lookups run in parallel; only a genuine miss
//! atomically upgrades that guard to the exclusive write lock to insert.

use parking_lot::{RwLock, RwLockUpgradableReadGuard};

use crate::shard_reader::ShardReader;
use crate::shard_write::ShardWrite;
use crate::sym::Sym;

/// One shard: interning state guarded by a `RwLock`.
///
/// `#[repr(align(128))]` keeps distinct shards on distinct cache lines to avoid
/// false sharing between their locks.
#[repr(align(128))]
pub(crate) struct Shard {
    state: RwLock<ShardWrite>,
}

impl Shard {
    pub(crate) fn new() -> Self {
        Self {
            state: RwLock::new(ShardWrite::new()),
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
        // insert without re-probing. Other threads can still `read()` this shard.
        let mut w = RwLockUpgradableReadGuard::upgrade(up);
        w.insert_new(idx, h, s, rehash)
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

    /// Snapshots this shard into a flat [`ShardReader`] without consuming it,
    /// copying its `(offsets, bytes)` blob under the lock. Used to freeze while the
    /// interner is still shared.
    pub(crate) fn snapshot(&self) -> ShardReader {
        let g = self.state.read();
        let (offsets, bytes) = g.parts();
        ShardReader::new(offsets.to_vec().into_boxed_slice(), bytes.to_vec().into_boxed_slice())
    }
}
