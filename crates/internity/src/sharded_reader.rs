// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ShardedReader`]: a frozen [`ThreadedLexicon`](crate::ThreadedLexicon), one flat blob per shard.

use alloc::boxed::Box;

use crate::reader::Reader;
use crate::shard_reader::ShardReader;
use crate::sym::{NUM_SHARDS, Sym};

/// A frozen concurrent interner: one flat [`ShardReader`] per shard, addressed by
/// the `Sym`'s `[shard|local]` partition. Returned as `impl Reader` from
/// [`ThreadedLexicon::freeze`](crate::ThreadedLexicon::freeze).
pub(crate) struct ShardedReader {
    shards: Box<[ShardReader; NUM_SHARDS]>,
}

impl ShardedReader {
    pub(crate) fn new(shards: Box<[ShardReader; NUM_SHARDS]>) -> Self {
        Self { shards }
    }
}

impl crate::reader::Sealed for ShardedReader {}

impl Reader for ShardedReader {
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
