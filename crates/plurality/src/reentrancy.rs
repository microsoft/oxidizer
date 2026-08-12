// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A non-allocating latch that makes reentrancy a property of the
//! implementation rather than a precondition on the caller.
//!
//! Ref: docs/implementation/reentrancy.md.

use core::cell::Cell;

/// Rejects nested entry into a region whose state is temporarily inconsistent.
///
/// The pool's growth paths release control to an allocator at a point where
/// pool state is not fit to be observed: a chunk's slot-index range is derived
/// but not yet published, or a directory vector is mutably borrowed. An
/// allocator that allocates from the same pool would reach that state through
/// an entirely safe call. The latch turns such an entry into a rejection at the
/// boundary, so no caller carries an unenforced obligation.
///
/// The pool's `!Sync` bound confines every latched region to one thread at a
/// time, so plain cell access is sufficient and costs a predictable branch.
pub(crate) struct ReentrancyLatch {
    held: Cell<bool>,
}

impl ReentrancyLatch {
    pub(crate) const fn new() -> Self {
        Self { held: Cell::new(false) }
    }

    /// Claims the region, or returns `None` if this is a nested entry.
    ///
    /// The returned token releases the claim when dropped, including while a
    /// panic unwinds through the region.
    #[inline]
    pub(crate) fn enter(&self) -> Option<LatchToken<'_>> {
        if self.held.replace(true) {
            None
        } else {
            Some(LatchToken { latch: self })
        }
    }

    /// Claims a region whose caller has already established non-reentrancy.
    ///
    /// Some regions are unreachable from inside themselves because an earlier
    /// check on the only path in has already refused the nested caller. Such a
    /// region latches to publish itself to readers, not to arbitrate entry, so
    /// it has no rejection to report.
    ///
    /// # Panics
    ///
    /// In debug builds, if the region is entered while already claimed.
    #[inline]
    pub(crate) fn hold(&self) -> LatchToken<'_> {
        let was_held = self.held.replace(true);
        debug_assert!(!was_held, "a latched region was entered from inside itself");
        LatchToken { latch: self }
    }

    /// `true` while a latched region is in progress.
    #[inline]
    pub(crate) fn is_held(&self) -> bool {
        self.held.get()
    }
}

/// Releases the claim taken by [`ReentrancyLatch::enter`] or
/// [`ReentrancyLatch::hold`].
pub(crate) struct LatchToken<'a> {
    latch: &'a ReentrancyLatch,
}

impl Drop for LatchToken<'_> {
    #[inline]
    fn drop(&mut self) {
        self.latch.held.set(false);
    }
}
