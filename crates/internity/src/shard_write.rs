// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ShardWrite`]: a shard's entire state (dedup table + flat byte buffer).
//!
//! Fully safe: strings are concatenated into a growable `buffer: Vec<u8>` with
//! CSR-style `offsets` (`offsets[i]` start, `offsets[i+1]` end of the `i`-th
//! string), exactly like the single-threaded [`Lexicon`](crate::Lexicon). Because
//! a [`ThreadedLexicon`](crate::ThreadedLexicon) is fill-then-freeze (no resolve while interning), the
//! buffer may reallocate freely — no handles or references into it are outstanding
//! during the fill phase.

use alloc::vec;
use alloc::vec::Vec;

use hashbrown::HashTable;

use crate::sym::{LOCAL_MASK, Sym};

/// A shard's state, guarded by the shard's mutex: the dedup hash table
/// (string hash → handle), the CSR-style string boundaries, and the concatenated
/// string bytes.
pub(crate) struct ShardWrite {
    dedup: HashTable<Sym>,
    /// String boundaries into `buffer`, CSR-style: `offsets[i]` start,
    /// `offsets[i+1]` end of the `i`-th string. Always starts with a `0` sentinel
    /// (`len() + 1` entries), so resolve is branch-free.
    offsets: Vec<u32>,
    /// All of this shard's interned strings concatenated.
    buffer: Vec<u8>,
}

impl ShardWrite {
    pub(crate) fn new() -> Self {
        Self {
            dedup: HashTable::new(),
            offsets: vec![0],
            buffer: Vec::new(),
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
    /// a duplicate handle. `rehash` recomputes a handle's hash when the dedup table
    /// grows.
    pub(crate) fn insert_new(&mut self, idx: usize, h: u64, s: &str, rehash: impl Fn(&[u8]) -> u64) -> Sym {
        let local0 = self.offsets.len() - 1;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "local0 is asserted below to fit in LOCAL_MASK (< 2^26)"
        )]
        let local0_u32 = local0 as u32;
        assert!(local0_u32 < LOCAL_MASK, "internity: shard {idx} capacity exceeded");
        let end = self
            .buffer
            .len()
            .checked_add(s.len())
            .and_then(|n| u32::try_from(n).ok())
            .expect("internity: shard bytes exceed u32");
        self.buffer.extend_from_slice(s.as_bytes());
        self.offsets.push(end);
        let sym = Sym::pack(idx, local0_u32 + 1);

        let offsets = &self.offsets;
        let buffer = &self.buffer;
        self.dedup.insert_unique(h, sym, |&sym| {
            rehash(Self::str_at(offsets, buffer, sym.local() as usize).as_bytes())
        });
        sym
    }

    /// Looks up `s` without interning it.
    #[inline]
    pub(crate) fn get(&self, h: u64, s: &str) -> Option<Sym> {
        let offsets = &self.offsets;
        let buffer = &self.buffer;
        self.dedup
            .find(h, |&sym| Self::str_at(offsets, buffer, sym.local() as usize) == s)
            .copied()
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
}
