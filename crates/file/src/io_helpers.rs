// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared I/O helpers for reading into [`BytesBuf`] buffers.

use std::io::{Read, Result};

use bytesbuf::BytesBuf;

/// Reads up to `len` bytes from `reader` directly into `buf`'s unfilled capacity,
/// avoiding a temporary `Vec` allocation.
pub fn read_into_bytesbuf(reader: &mut impl Read, buf: &mut BytesBuf, len: usize) -> Result<usize> {
    let unfilled = buf.first_unfilled_slice();
    let read_len = len.min(unfilled.len());

    // SAFETY: MaybeUninit<u8> has the same layout as u8.
    // We are passing uninitialized memory to the reader.
    // Since we know the reader is a file, this is safe in practice as the OS
    // writes to the buffer without reading it.
    // The read call writes `n` bytes; we only advance by `n` below.
    let dst = unsafe { core::slice::from_raw_parts_mut(unfilled.as_mut_ptr().cast::<u8>(), read_len) };
    let n = reader.read(dst)?;
    if n > 0 {
        // SAFETY: `n` bytes were just written by the read call.
        unsafe {
            buf.advance(n);
        }
    }
    Ok(n)
}
