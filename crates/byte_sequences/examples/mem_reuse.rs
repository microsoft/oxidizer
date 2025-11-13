// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how you can easily and cheaply reuse Sequences and parts of Sequences.
use byte_sequences::{NeutralMemoryPool, Sequence};
use bytes::{Buf, BufMut};

fn main() {
    // The neutral memory pool in a real application would be provided by the framework.
    let memory = NeutralMemoryPool::new();

    let hello_world = Sequence::copy_from_slice(b"Hello, world!", &memory);

    inspect_sequence(&hello_world);

    // Splitting up a sequence into sub-sequences is a cheap zero-copy operation.
    let hello = hello_world.slice(0..5);
    let world = hello_world.slice(7..12);

    inspect_sequence(&hello);
    inspect_sequence(&world);

    // You can glue the parts back together if you wish. Again, this is a cheap zero-copy operation.
    let hello_world_reconstructed = Sequence::from_sequences([hello, world]);

    inspect_sequence(&hello_world_reconstructed);

    // You can also shove existing sequences into a sequence builder that is in the process
    // of creating a new byte sequence. This is also a cheap zero-copy operation.
    let mut sequence_builder = memory.reserve(1024);

    sequence_builder.put(b"The quick brown fox says \"".as_slice());
    sequence_builder.append(hello_world_reconstructed);
    sequence_builder.put(b"\" and jumps over the lazy dog.".as_slice());

    let fox_story = sequence_builder.consume_all();

    inspect_sequence(&fox_story);
}

fn inspect_sequence(sequence: &Sequence) {
    let len = sequence.len();
    let mut chunk_lengths = Vec::new();

    // We need to mutate the sequence to slide our inspection window over it, so we clone it.
    // Cloning a sequence is a cheap zero-copy operation, do not hesitate to do it when needed.
    let mut sequence = sequence.clone();

    while sequence.has_remaining() {
        let chunk = sequence.chunk();
        chunk_lengths.push(chunk.len());

        // We have completed processing this chunk, all we wanted was to know its length.
        sequence.advance(chunk.len());
    }

    println!("Inspected a sequence of {len} bytes with chunk lengths: {chunk_lengths:?}");
}
