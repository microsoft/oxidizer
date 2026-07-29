// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use core::hash::BuildHasher;

use crate::{LocalLexicon, Reader, Sym};

/// A string-interning engine.
///
/// Generic code can use this trait without choosing between the crate's local
/// and concurrent engines. The trait is dyn-compatible, so an engine can also
/// be stored as a [`Box<dyn Lexicon>`](Lexicon).
///
/// # Concurrency
///
/// [`intern`](Lexicon::intern) takes `&mut self` so a single signature covers
/// both engines. This means generic code written against `Lexicon` fills an
/// interner sequentially, even for [`ThreadedLexicon`], whose inherent
/// `intern(&self)` supports concurrent interning from multiple threads through a
/// shared `&`. To fill a [`ThreadedLexicon`] concurrently, call its inherent
/// `intern` directly rather than through this trait.
///
/// ```
/// use internity::{Lexicon, LocalLexicon};
///
/// fn intern_name(lexicon: &mut impl Lexicon, name: &str) -> internity::Sym {
///     lexicon.intern(name)
/// }
///
/// let mut local = LocalLexicon::new();
/// let name = intern_name(&mut local, "name");
/// assert_eq!(local.resolve(name), "name");
///
/// let erased: Box<dyn Lexicon> = Box::new(local);
/// assert_eq!(erased.freeze().resolve(name), "name");
/// ```
///
/// [`ThreadedLexicon`]: crate::ThreadedLexicon
pub trait Lexicon {
    /// Intern `value`, returning its [`Sym`] handle.
    ///
    /// Takes `&mut self` for a uniform signature across engines; see the
    /// [trait-level concurrency note](Lexicon#concurrency) for why concurrent
    /// fill of a [`ThreadedLexicon`](crate::ThreadedLexicon) must use its
    /// inherent `intern(&self)` instead.
    fn intern(&mut self, value: &str) -> Sym;

    /// Intern the UTF-8 string in `bytes`, returning its [`Sym`] handle.
    ///
    /// Validates UTF-8 at most once per distinct *valid* byte sequence: this
    /// accepts raw bytes and runs the `str::from_utf8` check itself, but only
    /// when the bytes are not already interned. On a dedup hit the stored entry is
    /// byte-equal to `bytes` and was validated when first inserted, so the check is
    /// skipped — amortizing UTF-8 validation across duplicate inserts in the
    /// high-duplication workloads interning targets. Handles still resolve back to
    /// a checked `&str`.
    ///
    /// # Errors
    ///
    /// Returns a [`Utf8Error`](core::str::Utf8Error) if `bytes` is not valid UTF-8.
    /// Invalid bytes are never stored, so the check is skipped only for input that
    /// byte-matches an already-interned (hence valid) string; repeated invalid
    /// input re-validates and returns `Err` every time.
    fn intern_bytes(&mut self, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error>;

    /// Return the handle for `value` if it is already interned.
    #[must_use]
    fn get(&self, value: &str) -> Option<Sym>;

    /// Return the number of distinct interned strings.
    ///
    /// For a concurrent engine, the result need not represent a point-in-time
    /// snapshot when other handles are interning concurrently.
    #[must_use]
    fn len(&self) -> usize;

    /// Return `true` if no strings have been interned.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Consume this boxed engine, returning a boxed read-only [`Reader`] view.
    ///
    /// Concrete engines also provide an inherent `freeze` method that avoids
    /// dynamic dispatch.
    #[must_use]
    fn freeze(self: Box<Self>) -> Box<dyn Reader>;
}

impl<S: BuildHasher> Lexicon for LocalLexicon<S> {
    fn intern(&mut self, value: &str) -> Sym {
        Self::intern(self, value)
    }

    fn intern_bytes(&mut self, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error> {
        Self::intern_bytes(self, bytes)
    }

    fn get(&self, value: &str) -> Option<Sym> {
        Self::get(self, value)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn freeze(self: Box<Self>) -> Box<dyn Reader> {
        (*self).into_boxed_reader()
    }
}

#[cfg(feature = "std")]
impl<S: BuildHasher> Lexicon for crate::ThreadedLexicon<S> {
    fn intern(&mut self, value: &str) -> Sym {
        Self::intern(self, value)
    }

    fn intern_bytes(&mut self, bytes: &[u8]) -> Result<Sym, core::str::Utf8Error> {
        Self::intern_bytes(self, bytes)
    }

    fn get(&self, value: &str) -> Option<Sym> {
        Self::get(self, value)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn freeze(self: Box<Self>) -> Box<dyn Reader> {
        (*self).into_boxed_reader()
    }
}
