// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Buffer construction, writes, and standard I/O adapters.

use bytesbuf::mem;

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
