// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! View construction, reads, ranges, and lifetime handling.

use bytesbuf::mem;

// Shared fixture for integration tests and private unit tests.
#[path = "../src/mem/testing/test_block.rs"]
mod test_block;

mod test_memory;

mod views {
    mod get;
    #[cfg(feature = "std")]
    mod read;
    mod view;
}
