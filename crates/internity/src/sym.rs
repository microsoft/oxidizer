// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Sym`] handle and its bit-packed encoding.
//!
//! A `Sym` is a 4-byte, `Copy` handle: `Option<Sym>` is also 4 bytes thanks to
//! the `NonZeroU32` niche. Each interner encodes its own layout into the 32 bits —
//! a `[shard][local]` split for [`ThreadedLexicon`](crate::ThreadedLexicon), or a
//! dense 1-based index for [`Lexicon`](crate::Lexicon) — and only decodes its own
//! handles. The value is always non-zero (indices are stored 1-based).

use core::num::NonZeroU32;

/// Number of bits used to select a shard. Must satisfy `1 << SHARD_BITS == N`
/// (the shard count).
#[cfg(feature = "std")]
pub(crate) const SHARD_BITS: u32 = 6;

/// Number of shards.
#[cfg(feature = "std")]
pub(crate) const NUM_SHARDS: usize = 1 << SHARD_BITS;

/// Number of bits used for the per-shard index.
#[cfg(feature = "std")]
pub(crate) const LOCAL_BITS: u32 = 32 - SHARD_BITS;

/// Mask covering the per-shard index bits. Also the maximum 1-based index.
#[cfg(feature = "std")]
pub(crate) const LOCAL_MASK: u32 = (1 << LOCAL_BITS) - 1;

/// A compact handle to an interned string.
///
/// A `Sym` is 4 bytes and `Copy`, so it's cheap to store, pass, and use as a map
/// key. `Option<Sym>` is also 4 bytes, thanks to a niche, so optional handles cost
/// nothing extra.
///
/// # Equality and ordering
///
/// Within one interner, two strings are interned to the *same* `Sym` iff they are
/// equal, so comparing handles with `==` is an O(1) proxy for comparing the
/// strings, and `Hash`/`Eq` let you use `Sym` directly as a `HashMap` key.
///
/// # Cross-interner use
///
/// A `Sym` is only meaningful for the interner that produced it. Resolving one on
/// a different interner is range-checked and returns `None`
/// ([`Lexicon::try_resolve`](crate::Lexicon::try_resolve)) — never undefined
/// behavior — though a foreign handle that happens to be in range will resolve to
/// an unrelated string.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Sym(NonZeroU32);

impl Sym {
    /// Packs a shard index and a 1-based local index into a `Sym`.
    #[cfg(feature = "std")]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // XOR and OR are equivalent because shard and local bits never overlap.
    pub(crate) fn pack(shard: usize, local1: u32) -> Self {
        debug_assert!(shard < NUM_SHARDS);
        debug_assert!((1..=LOCAL_MASK).contains(&local1));
        #[expect(clippy::cast_possible_truncation, reason = "shard < NUM_SHARDS (64) always fits in u32")]
        let raw = ((shard as u32) << LOCAL_BITS) | local1;
        // `local1 >= 1` guarantees `raw` is non-zero.
        Self(NonZeroU32::new(raw).expect("local1 >= 1 guarantees a non-zero packed Sym"))
    }

    /// The owning shard index.
    #[cfg(feature = "std")]
    #[inline]
    pub(crate) fn shard(self) -> usize {
        (self.0.get() >> LOCAL_BITS) as usize
    }

    /// The raw 1-based per-shard index (the low [`LOCAL_BITS`]).
    ///
    /// Returns `0` when the low bits are zero, which never happens for a `Sym`
    /// this crate produced (packed indices are 1-based) but *can* for a foreign or
    /// crafted handle — callers resolving untrusted handles must treat `0` as
    /// out of range rather than subtracting.
    #[cfg(feature = "std")]
    #[inline]
    pub(crate) fn local1(self) -> u32 {
        self.0.get() & LOCAL_MASK
    }

    /// The 0-based per-shard index. Only valid for a `Sym` this crate produced
    /// (where the low bits are `>= 1`); for foreign handles use [`local1`](Sym::local1).
    #[cfg(feature = "std")]
    #[inline]
    pub(crate) fn local(self) -> u32 {
        self.local1() - 1
    }

    /// Packs a 0-based dense index into a `Sym` (single-threaded [`Lexicon`]).
    ///
    /// Stored 1-based so the value is non-zero. This encoding is distinct from
    /// the sharded one; each interner decodes only its own handles.
    #[inline]
    pub(crate) fn pack_dense(index: usize) -> Self {
        // `raw = index + 1` must be a non-zero `u32`. `try_from` bounds the index
        // to `u32`; `wrapping_add(1)` then yields 0 only for `index == u32::MAX`,
        // which `NonZeroU32::new` rejects — so a single range check plus one
        // non-zero check cover both conditions.
        let raw = u32::try_from(index)
            .ok()
            .and_then(|i| NonZeroU32::new(i.wrapping_add(1)))
            .expect("internity: dense index exceeds u32");
        Self(raw)
    }

    /// The 0-based dense index (single-threaded [`Lexicon`]).
    #[inline]
    pub(crate) fn dense(self) -> usize {
        (self.0.get() - 1) as usize
    }

    /// The handle's raw non-zero `u32` value.
    ///
    /// Useful for compact serialization or FFI. Round-trips through
    /// [`from_u32`](Sym::from_u32).
    #[inline]
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0.get()
    }

    /// Reconstructs a `Sym` from a raw `u32` (see [`as_u32`](Sym::as_u32)), or
    /// `None` if `raw == 0`.
    ///
    /// This only checks the niche; it does not validate that the handle belongs to
    /// any particular interner. Resolving still range-checks, so a reconstructed
    /// handle can never cause undefined behavior — use
    /// [`Lexicon::try_resolve`](crate::Lexicon::try_resolve) to resolve safely.
    #[inline]
    #[must_use]
    pub fn from_u32(raw: u32) -> Option<Self> {
        NonZeroU32::new(raw).map(Self)
    }
}

impl From<Sym> for u32 {
    /// Returns the handle's raw non-zero `u32` value (see [`Sym::as_u32`]).
    #[inline]
    fn from(sym: Sym) -> Self {
        sym.0.get()
    }
}

impl core::fmt::Debug for Sym {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Show the raw handle value: the `[shard][local]` split is meaningful only
        // for `ThreadedLexicon` handles, so printing it would mislead for the dense
        // `Lexicon` encoding.
        f.debug_tuple("Sym").field(&self.0.get()).finish()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_pack_unpack() {
        for shard in [0usize, 1, 5, 31, 63] {
            for local1 in [1u32, 2, 100, LOCAL_MASK] {
                let s = Sym::pack(shard, local1);
                assert_eq!(s.shard(), shard);
                assert_eq!(s.local(), local1 - 1);
            }
        }
    }
}
