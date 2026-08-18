// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Reader`] trait: a read-only, `Send + Sync` view of an interner.

use alloc::boxed::Box;

use crate::sym::Sym;

/// Sealing module: `Reader` extends `sealed::Sealed`, which only this crate can
/// implement, so `Reader` cannot be implemented downstream. This lets the crate
/// add methods to `Reader` without a breaking change.
mod sealed {
    pub trait Sealed {}
}

pub(crate) use sealed::Sealed;

/// A read-only view of an interner, optimized for fast lookups.
///
/// A `Reader` is `Send + Sync` and its [`resolve`](Reader::resolve) is lock-free,
/// so you can share it across threads (e.g. behind an `Arc`) and resolve handles
/// concurrently. Handles produced by the source interner stay valid.
///
/// [`LocalLexicon`](crate::LocalLexicon) implements this trait directly. Calling
/// [`LocalLexicon::freeze`](crate::LocalLexicon::freeze) or
/// [`ThreadedLexicon::freeze`](crate::ThreadedLexicon::freeze) returns an
/// immutable [`LocalReader`](crate::LocalReader) or
/// [`ThreadedReader`](crate::ThreadedReader) — concrete types, so a frozen table
/// can be stored by value. Bring this trait into scope to call its methods
/// (`use internity::Reader`), and use `impl Reader` / `Box<dyn Reader>` when you
/// need to abstract over reader implementations.
///
/// This trait is [sealed](https://rust-lang.github.io/api-guidelines/future-proofing.html#c-sealed):
/// it cannot be implemented outside this crate.
///
/// # Examples
///
/// ```
/// use internity::{LocalLexicon, Reader};
///
/// let mut lexicon = LocalLexicon::new();
/// let a = lexicon.intern("hello");
/// assert_eq!(Reader::resolve(&lexicon, a), "hello");
/// assert_eq!(Reader::try_resolve(&lexicon, a), Some("hello"));
/// ```
#[expect(clippy::allow_attributes, reason = "private_bounds does not fire on every supported compiler")]
#[allow(private_bounds, reason = "Reader is intentionally sealed against downstream implementations")]
pub trait Reader: Sealed + Send + Sync {
    /// Resolves a handle to its string, or `None` if out of range for this reader.
    ///
    /// The range check is a **memory-safety** bound, not provenance validation: a
    /// handle whose numeric index falls within this reader's range resolves
    /// successfully even if it originated from a different interner, returning
    /// whichever string occupies that slot here. Only out-of-range (including
    /// stale handles beyond the current length) handles return `None`.
    #[must_use]
    fn try_resolve(&self, sym: Sym) -> Option<&str>;

    /// Returns the number of distinct interned strings.
    #[must_use]
    fn len(&self) -> usize;

    /// Returns an iterator over `(Sym, &str)` for every interned string.
    ///
    /// For a [`LocalLexicon`](crate::LocalLexicon) or its frozen reader, the order
    /// is handle order; for a reader from
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
        self.try_resolve(sym).expect("internity: Sym is out of range for this reader")
    }

    /// Returns `true` if nothing was interned.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
