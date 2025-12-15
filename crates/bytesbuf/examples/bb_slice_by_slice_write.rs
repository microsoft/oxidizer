// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to write to a BytesBuf by directly writing into slices of mutable memory.
//!
//! Compared to the helper methods showcased in `bb_basic.rs`, writing via mutable slices is a more
//! advanced technique that gives you greater control over the writing process at the expense of
//! more complex code.
//!
//! See also `bb_reuse.rs`, which includes a complementary example of how to read slice-by-slice.

use std::ptr;

use bytesbuf::{BytesBuf, GlobalPool};

const LUCKY_NUMBER: usize = 8888;
const FAVORITE_COLOR: &[u8] = b"octarine";

fn main() {
    // The global memory pool in real-world code would be provided by the application framework.
    let memory = GlobalPool::new();

    // We emit the favorite color. For maximal happiness, we repeat it LUCKY_NUMBER times.
    let capacity_required = LUCKY_NUMBER * FAVORITE_COLOR.len();

    let mut buf = memory.reserve(capacity_required);

    for _ in 0..LUCKY_NUMBER {
        emit_favorite_color(&mut buf);
    }

    let message = buf.consume_all();

    println!(
        "Emitted favorite color {LUCKY_NUMBER} times, resulting in a message of {} bytes.",
        message.len()
    );
}

fn emit_favorite_color(buf: &mut BytesBuf) {
    assert!(
        buf.remaining_capacity() >= FAVORITE_COLOR.len(),
        "insufficient remaining capacity in buffer"
    );

    let mut payload_to_write = FAVORITE_COLOR;

    while !payload_to_write.is_empty() {
        // This returns a mutable slice of uninitialized memory that we can fill. However, there is
        // no guarantee on how many bytes this slice covers (it could be as little as 1 byte per slice).
        // We are required to fill all bytes before we can proceed to the next slice (which may also
        // be as small as 1 byte in length).
        let slice = buf.first_unfilled_slice();

        // It could be that we cannot write the entire payload to this slice, so we write as much
        // as we can and leave the remainder to the next slice.
        let bytes_to_write = slice.len().min(payload_to_write.len());
        let slice_to_write = &payload_to_write[..bytes_to_write];

        // Once write_copy_of_slice() is stabilized, we can replace this with a safe alternative.
        // SAFETY: Both pointers are valid for reading/writing bytes and we guard length via min().
        unsafe {
            ptr::copy_nonoverlapping(slice_to_write.as_ptr(), slice.as_mut_ptr().cast(), bytes_to_write);
        }

        // SAFETY: We must have actually initialized this many bytes. We did.
        unsafe {
            buf.advance(bytes_to_write);
        }

        // We wrote some (or all) of the payload, so keep whatever
        // is left for the next loop iteration if we did not write everything.
        payload_to_write = &payload_to_write[bytes_to_write..];
    }
}
