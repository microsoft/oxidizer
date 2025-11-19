// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to write to a sequence builder using mutable memory slices.
//!
//! Compared to the helper methods used in `mem_basic.rs`, writing via mutable slices is a more
//! advanced technique that gives you greater control over the writing process at the expense of
//! more complex code.
//!
//! See also `mem_reuse.rs`, which includes an example of how to read chunk-by-chunk.

use std::ptr;

use byte_sequences::{ByteSequenceBuilder, NeutralMemoryPool};
use bytes::BufMut;

const LUCKY_NUMBER: usize = 8888;
const FAVORITE_COLOR: &[u8] = b"octarine";

fn main() {
    // The neutral memory pool in a real application would be provided by the framework.
    let memory = NeutralMemoryPool::new();

    // We emit the favorite color. For maximal happiness, we repeat it LUCKY_NUMBER times.
    let capacity_required = LUCKY_NUMBER * FAVORITE_COLOR.len();

    let mut sequence_builder = memory.reserve(capacity_required);

    for _ in 0..LUCKY_NUMBER {
        emit_favorite_color(&mut sequence_builder);
    }

    let sequence = sequence_builder.consume_all();

    println!(
        "Emitted favorite color {LUCKY_NUMBER} times, resulting in a sequence of {} bytes.",
        sequence.len()
    );
}

fn emit_favorite_color(sequence_builder: &mut ByteSequenceBuilder) {
    assert!(sequence_builder.has_remaining_mut(), "no remaining capacity in sequence builder");

    let mut payload_to_write = FAVORITE_COLOR;

    while !payload_to_write.is_empty() {
        // This returns a mutable slice of uninitialized memory that we can fill. However, there is
        // no guarantee on how many bytes this slice covers (it could be as little as 1 byte per chunk).
        // We are required to fill all bytes before we can proceed to the next chunk (which may also
        // be as small as 1 byte in length).
        let chunk = sequence_builder.chunk_mut();

        // It could be that we cannot write the entire payload to this chunk, so we write as much
        // as we can and leave the remainder to the next chunk.
        let bytes_to_write = chunk.len().min(payload_to_write.len());
        let slice_to_write = &payload_to_write[..bytes_to_write];

        // Once write_copy_of_slice() is stabilized, we can replace this with a safe alternative.
        // SAFETY: Both pointers are valid for reading/writing bytes, all is well.
        unsafe {
            ptr::copy_nonoverlapping(slice_to_write.as_ptr(), chunk.as_mut_ptr(), bytes_to_write);
        }

        // SAFETY: We must have actually initialized this many bytes. We did.
        unsafe {
            sequence_builder.advance_mut(bytes_to_write);
        }

        // We wrote some (or all) of the payload, so keep whatever
        // is left for the next loop iteration.
        payload_to_write = &payload_to_write[bytes_to_write..];
    }
}
