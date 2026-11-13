// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::io::Write;

use bytesbuf::BytesBuf;
use new_zealand::nz;

use crate::test_memory::TestMemory;

#[test]
fn smoke_test_write_and_verify_data() {
    let test_data = b"Hello, world! This is a test.";

    let memory = TestMemory::new();
    let builder = memory.reserve(100);

    let mut write_adapter = builder.into_writer(&memory);

    // no-op
    write_adapter.flush().unwrap();

    let bytes_written = write_adapter.write(test_data).expect("write should succeed");
    assert_eq!(bytes_written, test_data.len());

    let mut builder = write_adapter.into_inner();
    let data = builder.consume_all();
    assert_eq!(data, test_data.as_slice());
}

#[test]
fn existing_content_is_preserved() {
    let memory = TestMemory::new();
    let mut builder = memory.reserve(100);

    // Add some initial content to the builder
    let initial_data = b"Initial content";
    builder.put_slice(initial_data.as_slice());

    let mut write_adapter = builder.into_writer(&memory);

    let additional_data = b" - Additional data";
    let bytes_written = write_adapter.write(additional_data).expect("write should succeed");
    assert_eq!(bytes_written, additional_data.len());

    let mut builder = write_adapter.into_inner();
    // Verify both initial and additional data are present
    let data = builder.consume_all();
    let expected = b"Initial content - Additional data";
    assert_eq!(data, expected.as_slice());
}

#[test]
fn sufficient_capacity_not_extended() {
    let test_data = b"Small data"; // Much smaller than 100 bytes

    let memory = TestMemory::with_block_size(nz!(1024));
    let builder = memory.reserve(100);

    let initial_capacity = builder.capacity();
    assert!(initial_capacity >= 100);

    let mut write_adapter = builder.into_writer(&memory);

    // Write data that fits within existing capacity
    let bytes_written = write_adapter.write(test_data).expect("write should succeed");
    assert_eq!(bytes_written, test_data.len());

    let mut builder = write_adapter.into_inner();

    // Verify capacity hasn't changed (no new allocation needed)
    assert_eq!(builder.capacity(), initial_capacity);

    // Verify the data is there
    let data = builder.consume_all();
    assert_eq!(data, test_data.as_slice());
}

#[test]
fn empty_buffer_small_write_reserves_initial_size() {
    let memory = TestMemory::new();
    let buf = BytesBuf::new();
    assert_eq!(buf.capacity(), 0);

    let mut writer = buf.into_writer(&memory);
    writer.write_all(b"hello").unwrap();

    let buf = writer.into_inner();
    assert!(buf.capacity() >= 4096, "expected capacity >= 4096, got {}", buf.capacity());
}

#[test]
fn empty_buffer_large_write_reserves_growth_size() {
    let memory = TestMemory::new();
    let buf = BytesBuf::new();

    let payload = vec![0u8; 4096 + 1];

    let mut writer = buf.into_writer(&memory);
    writer.write_all(&payload).unwrap();

    let buf = writer.into_inner();
    assert!(buf.capacity() >= 65536, "expected capacity >= 65536, got {}", buf.capacity());
}

#[test]
fn non_empty_buffer_reserves_growth_size() {
    let memory = TestMemory::new();
    let mut buf = memory.reserve(16);
    buf.put_slice([1; 16]);
    assert_eq!(buf.remaining_capacity(), 0);
    assert!(buf.capacity() > 0);

    let mut writer = buf.into_writer(&memory);
    writer.write_all(b"more data").unwrap();

    let buf = writer.into_inner();
    assert!(
        buf.capacity() >= 16 + 65536,
        "expected capacity >= {}, got {}",
        16 + 65536,
        buf.capacity()
    );
}
