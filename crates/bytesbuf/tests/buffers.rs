// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Buffer construction, writes, and standard I/O adapters.

use core::sync::atomic as sync_fence;

use bytesbuf::mem;

#[cfg(feature = "std")]
pub(crate) fn memory() -> &'static bytesbuf::mem::GlobalPool {
    static MEMORY: std::sync::OnceLock<bytesbuf::mem::GlobalPool> = std::sync::OnceLock::new();

    MEMORY.get_or_init(bytesbuf::mem::GlobalPool::new)
}

// Shared fixture for integration tests and private unit tests.
#[path = "../src/mem/testing/test_block.rs"]
mod test_block;

mod test_memory;

mod buffers {
    mod buf;
    mod put;
    #[cfg(feature = "std")]
    mod writer;
}
