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

    /// `true` while a latched region is in progress.
    #[inline]
    pub(crate) fn is_held(&self) -> bool {
        self.held.get()
    }
}

/// Releases the claim taken by [`ReentrancyLatch::enter`].
pub(crate) struct LatchToken<'a> {
    latch: &'a ReentrancyLatch,
}

impl Drop for LatchToken<'_> {
    #[inline]
    fn drop(&mut self) {
        self.latch.held.set(false);
    }
}
