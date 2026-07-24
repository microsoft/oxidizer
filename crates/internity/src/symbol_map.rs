// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fast [`Sym`](crate::Sym)-keyed maps and sets via an identity-style hasher.
//!
//! A [`Sym`](crate::Sym) is already a unique, small integer, so hashing it with a
//! general-purpose hasher is wasted work. [`SymBuildHasher`] instead turns the
//! handle straight into a well-distributed hash with a single multiply — giving
//! `HashMap`/`HashSet` keyed by `Sym` near-perfect, collision-free behavior.
//!
//! [`SymMap`] and [`SymSet`] are ready-made aliases (available with the `std`
//! feature). In `no_std`, use [`SymBuildHasher`] with any hash map, e.g.
//! `hashbrown::HashMap<Sym, V, SymBuildHasher>`.

use core::hash::{BuildHasherDefault, Hasher};

#[cfg(feature = "std")]
use crate::sym::Sym;

/// Golden-ratio multiplier; spreads a small integer across all 64 hash bits.
const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// A minimal [`Hasher`] specialized for [`Sym`](crate::Sym) keys.
///
/// `Sym`'s `Hash` writes a single `u32`, which this turns into a hash with one
/// multiply — no byte-wise mixing. Prefer the [`SymBuildHasher`] alias (and the
/// `SymMap` / `SymSet` aliases with the `std` feature) over naming this directly.
#[derive(Debug, Default)]
pub struct SymHasher(u64);

impl Hasher for SymHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        // Accumulate rather than overwrite so composite keys (multiple `write_u32`
        // calls) mix correctly; identical to a plain multiply for the single-write
        // `Sym` case since the initial state is 0.
        self.0 = (self.0 ^ u64::from(i)).wrapping_mul(MIX);
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Fallback path: `Sym` only ever calls `write_u32`, but a correct `Hasher`
        // must handle arbitrary input, so fold the bytes with the same multiply.
        let mut h = self.0;
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(MIX);
        }
        self.0 = h;
    }
}

/// A [`BuildHasher`](core::hash::BuildHasher) that produces [`SymHasher`]s, for
/// fast [`Sym`](crate::Sym)-keyed maps and sets.
pub type SymBuildHasher = BuildHasherDefault<SymHasher>;

/// A [`HashMap`](std::collections::HashMap) keyed by [`Sym`](crate::Sym) using the fast
/// [`SymBuildHasher`].
#[cfg(feature = "std")]
pub type SymMap<V> = std::collections::HashMap<Sym, V, SymBuildHasher>;

/// A [`HashSet`](std::collections::HashSet) of [`Sym`](crate::Sym) using the fast
/// [`SymBuildHasher`].
#[cfg(feature = "std")]
pub type SymSet = std::collections::HashSet<Sym, SymBuildHasher>;

#[cfg(test)]
mod tests {
    use core::hash::Hasher;

    use super::{MIX, SymHasher};

    #[test]
    fn write_u32_accumulates_and_mixes() {
        let mut hasher = SymHasher::default();
        hasher.write_u32(1);
        let first = MIX;
        assert_eq!(hasher.finish(), first);

        hasher.write_u32(1);
        assert_eq!(hasher.finish(), (first ^ 1).wrapping_mul(MIX));
    }

    #[test]
    fn write_accumulates_and_mixes_each_byte() {
        let mut hasher = SymHasher::default();
        hasher.write(&[0xff, 0x55]);

        let first = 0xff_u64.wrapping_mul(MIX);
        assert_eq!(hasher.finish(), (first ^ 0x55).wrapping_mul(MIX));
    }
}
