// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use smallvec::SmallVec;

use crate::{BlockRef, MAX_INLINE_SPANS};

/// Guards memory blocks with a liveness lock, preventing a set of memory blocks
/// from being released while the guard is alive.
///
/// Call [`ByteSequence::extend_lifetime()`][1] or [`ByteSequenceBuilder::extend_lifetime()`][2] to obtain
/// an instance.
///
/// The memory may be used for any otherwise legal purpose; all this guard does is act as a
/// shadow reference to the memory blocks. This can be useful when executing unsafe logic,
/// where there may not otherwise exist any Rust objects holding references to memory blocks
/// in use (e.g. because the code operating on them is not even Rust code).
///
/// [1]: crate::ByteSequence::extend_lifetime
/// [2]: crate::ByteSequenceBuilder::extend_lifetime
#[derive(Debug)]
#[must_use]
pub struct MemoryGuard {
    _block_refs: SmallVec<[BlockRef; MAX_INLINE_SPANS]>,
}

impl MemoryGuard {
    /// Creates a new memory guard for the provided memory blocks.
    pub(crate) fn new(block_refs: impl IntoIterator<Item = BlockRef>) -> Self {
        Self {
            _block_refs: block_refs.into_iter().collect(),
        }
    }
}

impl Default for MemoryGuard {
    /// Creates a memory guard that does not guard any memory blocks.
    ///
    /// Useless for real logic but potentially meaningful as a placeholder in tests.
    fn default() -> Self {
        Self::new(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(clippy::used_underscore_binding, reason = "Sometimes, you gotta do what you gotta do.")]
    #[test]
    fn default_creates_empty_guard() {
        let guard = MemoryGuard::default();
        assert!(guard._block_refs.is_empty());
    }
}
