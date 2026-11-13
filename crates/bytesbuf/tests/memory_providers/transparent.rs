// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytesbuf::mem::MemoryShared;
use bytesbuf::mem::testing::TransparentMemory;
use static_assertions::assert_impl_all;
use thread_aware::ThreadAware;

assert_impl_all!(TransparentMemory: MemoryShared, ThreadAware);

#[test]
fn smoke_test() {
    let memory = TransparentMemory::new();

    let mut buf = memory.reserve(0);
    assert_eq!(buf.len(), 0);
    assert_eq!(buf.capacity(), 0);

    buf = memory.reserve(1313);

    assert_eq!(buf.capacity(), 1313);

    buf.put_byte_repeated(3, 1313);

    let data = buf.consume_all();

    assert_eq!(data.len(), 1313);
    assert_eq!(data.first_slice().len(), 1313);
}

#[test]
fn giant_allocation() {
    // This test requires at least 5 GB of memory to run. The publishing pipeline runs on a system
    // where this may not be available, so we skip this test in that environment.
    #[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
    if crate::support::system_memory() < 10_000_000_000 {
        eprintln!("Skipping giant allocation test due to insufficient memory.");
        return;
    }

    // This is a giant allocation that does not fit into one memory block.

    let memory = TransparentMemory::new();
    let buf = memory.reserve(5_000_000_000);

    assert_eq!(buf.capacity(), 5_000_000_000);

    // NB! We cannot simply check the first chunk length because there is no guarantee on which
    // order a BytesBuf consumes its blocks in - the first might not be u32::MAX here!
}
