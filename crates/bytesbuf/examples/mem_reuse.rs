// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how you can easily and cheaply reuse byte sequences and parts of byte sequences.
use bytes::Buf;
use bytesbuf::{BytesView, GlobalPool};

fn main() {
    // The global memory pool in a real application would be provided by the framework.
    let memory = GlobalPool::new();

    let hello_world = BytesView::copied_from_slice(b"Hello, world!", &memory);

    inspect_sequence(&hello_world);

    // Splitting up a sequence into sub-sequences is a cheap zero-copy operation.
    let hello = hello_world.range(0..5);
    let world = hello_world.range(7..12);

    inspect_sequence(&hello);
    inspect_sequence(&world);

    // You can glue the parts back together if you wish. Again, this is a cheap zero-copy operation.
    let hello_world_reconstructed = BytesView::from_views([hello, world]);

    inspect_sequence(&hello_world_reconstructed);

    // You can also shove existing sequences into a sequence builder that is in the process
    // of creating a new byte sequence. This is also a cheap zero-copy operation.
    let mut sequence_builder = memory.reserve(1024);

    sequence_builder.put_slice(b"The quick brown fox says \"".as_slice());
    sequence_builder.put_view(hello_world_reconstructed);
    sequence_builder.put_slice(b"\" and jumps over the lazy dog.".as_slice());

    let fox_story = sequence_builder.consume_all();

    inspect_sequence(&fox_story);
}

fn inspect_sequence(sequence: &BytesView) {
    let len = sequence.len();
    let mut chunk_lengths = Vec::new();

    // We need to mutate the sequence to slide our inspection window over it, so we clone it.
    // Cloning a sequence is a cheap zero-copy operation, do not hesitate to do it when needed.
    let mut sequence = sequence.clone();

    while sequence.has_remaining() {
        let chunk = sequence.first_slice();
        chunk_lengths.push(chunk.len());

        // We have completed processing this chunk, all we wanted was to know its length.
        sequence.advance(chunk.len());
    }

    println!("Inspected a sequence of {len} bytes with chunk lengths: {chunk_lengths:?}");
}
