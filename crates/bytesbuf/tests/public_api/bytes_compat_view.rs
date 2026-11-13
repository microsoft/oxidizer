// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::io::IoSlice;

use bytes::Buf;
use bytesbuf::mem::testing::FixedBlockMemory;
use new_zealand::nz;

#[test]
fn buf_compat() {
    let memory = FixedBlockMemory::new(nz!(25));

    // 25 x 4
    let mut buf = memory.reserve(100);
    buf.put_byte_repeated(0x44, 100);

    let mut bytes = buf.consume_all();

    assert_eq!(Buf::remaining(&bytes), 100);

    let chunk = Buf::chunk(&bytes);
    assert_eq!(chunk.len(), 25);
    assert_eq!(chunk, &[0x44; 25]);

    Buf::advance(&mut bytes, 20);

    let chunk = Buf::chunk(&bytes);
    assert_eq!(chunk.len(), 5);
    assert_eq!(chunk, &[0x44; 5]);

    Buf::advance(&mut bytes, 5);

    let chunk = Buf::chunk(&bytes);
    assert_eq!(chunk.len(), 25);
    assert_eq!(chunk, &[0x44; 25]);

    Buf::advance(&mut bytes, 5);

    let mut io_slices = [IoSlice::new(&[]); 4];
    let n = Buf::chunks_vectored(&bytes, &mut io_slices);

    // We have already advanced past the first 30 bytes
    // but the remaining 70 should still be here for us as 20 + 25 + 25.
    assert_eq!(n, 3);

    assert_eq!(&*io_slices[0], &[0x44; 20]);
    assert_eq!(&*io_slices[1], &[0x44; 25]);
    assert_eq!(&*io_slices[2], &[0x44; 25]);
}

#[test]
fn chunks_vectored_empty_dst() {
    let memory = FixedBlockMemory::new(nz!(25));
    let mut buf = memory.reserve(25);
    buf.put_byte_repeated(0x44, 25);

    let bytes = buf.consume_all();
    let n = Buf::chunks_vectored(&bytes, &mut []);
    assert_eq!(n, 0);
}
