// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basics of working with `BytesBuf` and `BytesView`.
//!
//! 1. We creates a `BytesBuf` with some memory capacity.
//! 2. We encode a message into this buffer and create a `BytesView` over the message.
//! 3. We receive the message in another function and write a status report to the terminal.

use bytesbuf::BytesView;
use bytesbuf::mem::{GlobalPool, Memory};

fn main() {
    // The global memory pool in real-world code would be provided by the application framework.
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
    let mut buf = memory.reserve(MESSAGE_LEN_BYTES);

    println!(
        "Requested {MESSAGE_LEN_BYTES} bytes of memory capacity, got {} bytes.",
        buf.capacity()
    );

    // Each word is just an incrementing binary-serialized number, starting from 0.
    (0..MESSAGE_LEN_WORDS).for_each(|word| {
        buf.put_num_le(word as u64);
    });

    // Creates a BytesView over all the data written into the BytesBuf.
    buf.consume_all()
}

fn consume_message(mut message: BytesView) {
    // We read the message and calculate the sum of all the words in it.
    let mut sum: u64 = 0;

    while !message.is_empty() {
        let word = message.get_num_le::<u64>();
        sum = sum.saturating_add(word);
    }

    println!("Message received. The sum of all words in the message is {sum}.");
}
