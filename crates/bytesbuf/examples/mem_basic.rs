// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basics of working with byte sequences. We obtain some memory, encode a message into it,
//! and then receive this message in another function and write a status report to the terminal.

use bytes::{Buf, BufMut};
use bytesbuf::{BytesView, GlobalPool, Memory};

fn main() {
    // The global memory pool in a real application would be provided by the framework.
    let memory = GlobalPool::new();

    let message = produce_message(&memory);
    consume_message(message);
}

fn produce_message(memory: &impl Memory) -> BytesView {
    // Our message consists of this many "words" of data.
    const MESSAGE_LEN_WORDS: usize = 123_456;

    // Each word is a 64-bit integer, so this comes to a little under 1 MB of data.
    const MESSAGE_LEN_BYTES: usize = MESSAGE_LEN_WORDS * size_of::<u64>();

    // Reserve enough memory for the message. The memory provider may provide more than requested.
    let mut sequence_builder = memory.reserve(MESSAGE_LEN_BYTES);

    println!(
        "Requested {MESSAGE_LEN_BYTES} bytes of memory capacity, got {} bytes.",
        sequence_builder.capacity()
    );

    // Each word is just an incrementing number, starting from 0.
    (0..MESSAGE_LEN_WORDS).for_each(|word| {
        sequence_builder.put_u64(word as u64);
    });

    // Detaches a sequence of immutable bytes from the builder, consisting of all the data
    // written into it so far. The builder remains usable for further writes into any remaining
    // capacity, although we do not make use of that functionality here.
    sequence_builder.consume_all()
}

fn consume_message(mut message: BytesView) {
    // We read the message and calculate the sum of all the words in it.
    let mut sum: u64 = 0;

    while message.has_remaining() {
        let word = message.get_u64();
        sum = sum.saturating_add(word);
    }

    println!("Message received. The sum of all words in the message is {sum}.");
}
