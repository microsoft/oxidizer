// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests byte sequences through public APIs and existing test fixtures.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use bytesbuf::mem;

#[path = "../src/mem/testing/test_block.rs"]
mod test_block;
#[path = "support/system_memory.rs"]
mod testing;

#[path = "public_api/buf.rs"]
mod buf;
#[path = "public_api/buf_put.rs"]
mod buf_put;
#[path = "public_api/buf_writer.rs"]
mod buf_writer;
#[cfg(feature = "bytes-compat")]
#[path = "public_api/bytes_compat_buf.rs"]
mod bytes_compat_buf;
#[cfg(feature = "bytes-compat")]
#[path = "public_api/bytes_compat_from_bytes.rs"]
mod bytes_compat_from_bytes;
#[cfg(feature = "bytes-compat")]
#[path = "public_api/bytes_compat_to_bytes.rs"]
mod bytes_compat_to_bytes;
#[cfg(feature = "bytes-compat")]
#[path = "public_api/bytes_compat_view.rs"]
mod bytes_compat_view;
#[path = "public_api/mem_block_ref.rs"]
mod mem_block_ref;
#[path = "public_api/mem_callback_memory.rs"]
mod mem_callback_memory;
#[path = "public_api/mem_global.rs"]
mod mem_global;
#[path = "public_api/mem_memory.rs"]
mod mem_memory;
#[path = "public_api/mem_opaque_memory.rs"]
mod mem_opaque_memory;
#[path = "public_api/mem_testing_fixed_block.rs"]
mod mem_testing_fixed_block;
#[path = "public_api/mem_testing_transparent.rs"]
mod mem_testing_transparent;
#[path = "public_api/vec.rs"]
mod vec;
#[path = "public_api/view.rs"]
mod view;
#[path = "public_api/view_get.rs"]
mod view_get;
#[path = "public_api/view_read.rs"]
mod view_read;
