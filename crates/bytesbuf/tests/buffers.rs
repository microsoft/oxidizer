// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Buffer construction, writes, and standard I/O adapters.

use core::sync::atomic as sync_fence;

use bytesbuf::mem;

#[cfg(feature = "std")]
pub(crate) static MEMORY: std::sync::LazyLock<bytesbuf::mem::GlobalPool> = std::sync::LazyLock::new(bytesbuf::mem::GlobalPool::new);

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
