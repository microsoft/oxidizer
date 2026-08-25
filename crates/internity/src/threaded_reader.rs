// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ThreadedReader`]: a frozen [`ThreadedLexicon`](crate::ThreadedLexicon), one flat blob per shard.

use alloc::boxed::Box;

use crate::reader::Reader;
use crate::shard_reader::ShardReader;
use crate::sym::{NUM_SHARDS, Sym};

/// A frozen [`ThreadedLexicon`](crate::ThreadedLexicon): one flat blob per shard,
/// addressed by the `Sym`'s `[shard|local]` partition.
///
/// Returned by [`ThreadedLexicon::freeze`](crate::ThreadedLexicon::freeze).
///
/// # No dense index conversions
///
/// This reader offers no `index_of`/`sym_at` pair. Handles are shard-partitioned —
/// the high bits select one of the shards, so the values are spread across the
/// handle space rather than running consecutively from zero — and a dense position
/// cannot be recovered from one in constant time.
///
/// To read a corpus, use [`resolve`](Reader::resolve) for a single handle or
/// [`iter`](Reader::iter) to walk every entry. If you need positions for a `Vec<T>`
/// side table, intern with [`LocalLexicon`](crate::LocalLexicon) instead and use
/// [`LocalReader::index_of`](crate::LocalReader::index_of); the numbering it
/// guarantees is what makes those tables possible.
#[derive(Clone)]
pub struct ThreadedReader {
    shards: Box<[ShardReader; NUM_SHARDS]>,
}

impl ThreadedReader {
    pub(crate) fn new(shards: Box<[ShardReader; NUM_SHARDS]>) -> Self {
        Self { shards }
    }
}

impl core::fmt::Debug for ThreadedReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThreadedReader")
            .field("len", &Reader::len(self))
            .finish_non_exhaustive()
    }
}

impl crate::reader::Sealed for ThreadedReader {}

impl Reader for ThreadedReader {
    #[inline]
    fn try_resolve(&self, sym: Sym) -> Option<&str> {
        // `sym.shard()` is always in `0..NUM_SHARDS`, so the shard index needs no
        // bounds check. A foreign/crafted handle may have zero low bits, so decode
        // the 1-based local via `checked_sub` (→ `None`) rather than underflowing;
        // `ShardReader::get` range-checks the rest.
        let local = sym.local1().checked_sub(1)?;
        self.shards[sym.shard()].get(local as usize)
    }

    #[inline]
    fn len(&self) -> usize {
        self.shards.iter().map(ShardReader::len).sum()
    }

    fn iter(&self) -> Box<dyn Iterator<Item = (Sym, &str)> + '_> {
        Box::new(self.shards.iter().enumerate().flat_map(|(shard, reader)| {
            (0..reader.len()).map(move |local| {
                let s = reader.get(local).expect("local comes from 0..reader.len()");
                #[expect(clippy::cast_possible_truncation, reason = "local < reader.len() <= LOCAL_MASK fits in u32")]
                let sym = Sym::pack(shard, local as u32 + 1);
                (sym, s)
            })
        }))
    }
}
