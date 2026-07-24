// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Reader`] trait: a frozen, read-only, `Send + Sync` view of an interner.

use alloc::boxed::Box;

use crate::sym::Sym;

/// Sealing module: `Reader` extends `sealed::Sealed`, which only this crate can
/// implement, so `Reader` cannot be implemented downstream. This lets the crate
/// add methods to `Reader` without a breaking change.
mod sealed {
    pub trait Sealed {}
}

pub(crate) use sealed::Sealed;

/// A frozen, read-only view of an interner, optimized for fast lookups.
///
/// A `Reader` is `Send + Sync` and its [`resolve`](Reader::resolve) is lock-free,
/// so you can share it across threads (e.g. behind an `Arc`) and resolve handles
/// concurrently. Handles produced by the source interner stay valid.
///
/// Obtain one from [`Lexicon::freeze`](crate::Lexicon::freeze) or
/// [`ThreadedLexicon::freeze`](crate::ThreadedLexicon::freeze). Both return
/// `impl Reader`, so bring this trait into scope to call its methods
/// (`use internity::Reader`), and use `impl Reader` / `Box<dyn Reader>` if you need
/// to name the returned type.
///
/// This trait is [sealed](https://rust-lang.github.io/api-guidelines/future-proofing.html#c-sealed):
/// it cannot be implemented outside this crate.
///
/// # Examples
///
/// ```
/// use internity::{Lexicon, Reader};
///
/// let mut lexicon = Lexicon::new();
/// let a = lexicon.intern("hello");
/// let reader = lexicon.freeze();
/// assert_eq!(reader.resolve(a), "hello");
/// assert_eq!(reader.try_resolve(a), Some("hello"));
/// ```
pub trait Reader: Sealed + Send + Sync {
    /// Resolves a handle to its string, or `None` if it is out of range for this
    /// reader. This range check makes resolving a foreign or stale handle safe.
    #[must_use]
    fn try_resolve(&self, sym: Sym) -> Option<&str>;

    /// Returns the number of distinct interned strings.
    #[must_use]
    fn len(&self) -> usize;

    /// Returns an iterator over `(Sym, &str)` for every interned string.
    ///
    /// For a reader from [`Lexicon::freeze`](crate::Lexicon::freeze) the order is
    /// handle order; for one from
    /// [`ThreadedLexicon::freeze`](crate::ThreadedLexicon::freeze) it is grouped by
    /// shard.
    #[must_use]
    fn iter(&self) -> Box<dyn Iterator<Item = (Sym, &str)> + '_>;

    /// Resolves a handle to its string.
    ///
    /// # Panics
    ///
    /// Panics if `sym` is out of range for this reader. Use
    /// [`try_resolve`](Reader::try_resolve) for a non-panicking check.
    #[inline]
    #[must_use]
    fn resolve(&self, sym: Sym) -> &str {
        self.try_resolve(sym).expect("internity: Sym does not belong to this reader")
    }

    /// Returns `true` if nothing was interned.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
