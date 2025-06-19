// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::Block;

/// Guards I/O memory with a liveness lock, preventing a set of I/O blocks it from being released
/// back to the pool while the guard is alive.
///
/// The memory may be used for any otherwise legal purpose; all this guard does is act as a
/// shadow reference to some I/O blocks, held by the I/O subsystem during an I/O operation, thereby
/// ensuring that the memory used in that operation does not get released even if the caller
/// releases all references to the memory and the I/O operation.
#[derive(Debug)]
pub struct MemoryGuard {
    _blocks: Vec<Arc<Block>>,
}

impl MemoryGuard {
    /// Creates a new memory guard for the given I/O blocks.
    pub(crate) fn new(blocks: impl IntoIterator<Item = Arc<Block>>) -> Self {
        Self {
            _blocks: blocks.into_iter().collect(),
        }
    }
}

impl Default for MemoryGuard {
    fn default() -> Self {
        Self::new(vec![])
    }
}