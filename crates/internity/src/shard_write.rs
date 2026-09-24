// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ShardWrite`]: a shard's entire state (dedup table + flat byte buffer).
//!
//! Fully safe: strings are concatenated into a growable `buffer: Vec<u8>` with
//! CSR-style `offsets` (`offsets[i]` start, `offsets[i+1]` end of the `i`-th
//! string), exactly like the single-threaded [`LocalLexicon`](crate::LocalLexicon). Because
//! a [`ThreadedLexicon`](crate::ThreadedLexicon) is fill-then-freeze (no resolve while interning), the
//! buffer may reallocate freely — no handles or references into it are outstanding
//! during the fill phase.

use alloc::vec::Vec;

use hashbrown::HashTable;

use crate::sym::{LOCAL_MASK, Sym};

/// A shard's state, guarded by the shard's `RwLock`: the dedup hash table
/// (string hash → handle), the CSR-style string boundaries, and the concatenated
/// string bytes.
pub(crate) struct ShardWrite {
    dedup: HashTable<Entry>,
    /// String boundaries into `buffer`, CSR-style: `offsets[i]` start,
    /// `offsets[i+1]` end of the `i`-th string. Always starts with a `0` sentinel
    /// (`len() + 1` entries), so resolve is branch-free.
    offsets: Vec<u32>,
    /// All of this shard's interned strings concatenated.
    buffer: Vec<u8>,
}

struct Entry {
    hash: u64,
    sym: Sym,
}

struct StorageRollback<'a> {
    offsets: &'a mut Vec<u32>,
    buffer: &'a mut Vec<u8>,
    local: usize,
}

impl Drop for StorageRollback<'_> {
    fn drop(&mut self) {
        self.offsets.truncate(self.local + 1);
        self.buffer.truncate(self.offsets[self.local] as usize);
    }
}

impl ShardWrite {
    fn checked_local(local: usize) -> Option<u32> {
        (local < LOCAL_MASK as usize).then(|| u32::try_from(local).expect("local is below LOCAL_MASK"))
    }

    pub(crate) fn with_capacity(strings: usize, bytes: usize) -> Self {
        let mut offsets = Vec::with_capacity(strings.saturating_add(1));
        offsets.push(0);
        Self {
            dedup: HashTable::with_capacity(strings),
            offsets,
            buffer: Vec::with_capacity(bytes),
        }
    }

    /// Number of strings interned in this shard.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    /// The string at `local`, which comes from a handle stored in `dedup`.
    #[inline]
    fn str_at<'a>(offsets: &[u32], buffer: &'a [u8], local: usize) -> &'a str {
        crate::storage::str_at(offsets, buffer, local)
    }

    /// Inserts `s` (hash `h`) as a **new** string and returns its handle.
    ///
    /// The caller must have already established that `s` is absent (e.g. via
    /// [`get`](Self::get) under a lock held continuously through the upgrade), so
    /// this skips the dedup re-probe. Calling it for a present string would create
    /// a duplicate handle. Stored hashes let table growth avoid invoking a
    /// user-supplied hasher while the shard's write lock is held.
    pub(crate) fn insert_new(&mut self, idx: usize, h: u64, s: &str) -> Sym {
        let local0 = self.offsets.len() - 1;
        let local0_u32 = Self::checked_local(local0).expect("internity: caller cannot intern after this shard reaches LOCAL_MASK entries");
        let buffer_len = self.buffer.len();
        let end = crate::storage::checked_end(buffer_len, s.len()).expect("internity: shard bytes exceed u32");

        let storage = StorageRollback {
            offsets: &mut self.offsets,
            buffer: &mut self.buffer,
            local: local0,
        };
        storage.buffer.extend_from_slice(s.as_bytes());
        storage.offsets.push(end);
        let sym = Sym::pack(idx, local0_u32 + 1);

        // Rehash uses stored hashes, so a user-supplied hasher cannot run under the lock.
        self.dedup.insert_unique(h, Entry { hash: h, sym }, |entry| entry.hash);
        core::mem::forget(storage);
        sym
    }

    /// Looks up `s` without interning it.
    #[inline]
    pub(crate) fn get(&self, h: u64, s: &str) -> Option<Sym> {
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        self.dedup
            .find(h, |entry| Self::str_at(offsets, buffer, entry.sym.local() as usize) == s)
            .map(|entry| entry.sym)
    }

    /// Looks up the raw byte sequence `bytes` without interning it.
    ///
    /// A match proves `bytes` equals an already-interned (hence valid UTF-8)
    /// string, which lets [`intern_bytes`](crate::ThreadedLexicon::intern_bytes)
    /// skip re-validating the input on a hit.
    #[inline]
    pub(crate) fn get_bytes(&self, h: u64, bytes: &[u8]) -> Option<Sym> {
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        self.dedup
            .find(h, |entry| {
                Self::str_at(offsets, buffer, entry.sym.local() as usize).as_bytes() == bytes
            })
            .map(|entry| entry.sym)
    }

    /// Consumes the write state, yielding the flat `(offsets, bytes)` blob for a
    /// [`ShardReader`](crate::shard_reader::ShardReader).
    pub(crate) fn into_parts(self) -> (Vec<u32>, Vec<u8>) {
        (self.offsets, self.buffer)
    }

    /// Borrows the flat `(offsets, bytes)` blob for a snapshot without consuming.
    pub(crate) fn parts(&self) -> (&[u32], &[u8]) {
        (&self.offsets, &self.buffer)
    }

    #[cfg(test)]
    pub(crate) fn capacities(&self) -> (usize, usize, usize) {
        (self.dedup.capacity(), self.offsets.capacity(), self.buffer.capacity())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::{LOCAL_MASK, ShardWrite, StorageRollback};

    #[test]
    fn local_index_limit_requires_room_for_one_based_index() {
        assert_eq!(ShardWrite::checked_local(LOCAL_MASK as usize - 1), Some(LOCAL_MASK - 1));
        assert_eq!(ShardWrite::checked_local(LOCAL_MASK as usize), None);
    }

    #[test]
    fn rollback_after_partial_insert_restores_shard_storage() {
        let mut shard = ShardWrite::with_capacity(0, 0);
        {
            let storage = StorageRollback {
                offsets: &mut shard.offsets,
                buffer: &mut shard.buffer,
                local: 0,
            };
            storage.buffer.extend_from_slice(b"orphan");
            storage.offsets.push(6);
        }
        assert_eq!(shard.parts(), (&[0][..], &[][..]));

        let sym = shard.insert_new(0, 0, "live");
        assert_eq!(shard.get(0, "live"), Some(sym));
        assert_eq!(shard.parts(), (&[0, 4][..], &b"live"[..]));
    }
}
