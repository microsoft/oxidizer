// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to implement the `HasMemory` trait using the forwarding
//! implementation strategy, whereby the memory provider of a dependency is used.

use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

const PAYLOAD_LEN: usize = 12345;

fn main() {
    let connection = Connection::accept();

    let mut zero_counter = ConnectionZeroCounter::new(connection);

    let payload = create_payload(&zero_counter.memory());
    zero_counter.write(payload);

    println!("Sent {PAYLOAD_LEN} bytes, of which {} were 0x00 bytes.", zero_counter.zero_count());
}

fn create_payload(memory: &impl Memory) -> BytesView {
    let mut buf = memory.reserve(PAYLOAD_LEN);

    // We write PAYLOAD_LEN bytes, incrementing the value
    // by 1 each time, so we get [0, 1, 2, 3, ...].
    for i in 0..PAYLOAD_LEN {
        #[expect(clippy::cast_possible_truncation, reason = "intentional")]
        buf.put_byte(i as u8);
    }

    buf.consume_all()
}

/// Counts the number of 0x00 bytes in a byte sequence before
/// writing that byte sequence to a network connection.
///
/// # Implementation strategy for `HasMemory`
///
/// This type merely inspects a byte sequence before passing it on. This means that it does not
/// have a preference of its own for how that memory should be configured.
///
/// However, the thing it passes the sequence to (the `Connection` type) may have a preference,
/// so we forward the memory provider of the `Connection` type as our own memory provider, so the
/// caller can use memory optimal for submission to the `Connection` instance.
#[derive(Debug)]
struct ConnectionZeroCounter {
    connection: Connection,

    zero_count: u64,
}

impl ConnectionZeroCounter {
    pub const fn new(connection: Connection) -> Self {
        Self { connection, zero_count: 0 }
    }

    pub fn write(&mut self, message: BytesView) {
        // Cloning a BytesView is a cheap zero-copy operation,
        self.count_zeros(message.clone());

        self.connection.write(message);
    }

    fn count_zeros(&mut self, mut message: BytesView) {
        while !message.is_empty() {
            if message.get_byte() == 0 {
                self.zero_count = self.zero_count.wrapping_add(1);
            }
        }
    }

    pub const fn zero_count(&self) -> u64 {
        self.zero_count
    }
}

impl HasMemory for ConnectionZeroCounter {
    fn memory(&self) -> impl MemoryShared {
        // We forward the memory provider of the connection, so that the caller can use
        // memory optimal for submission to the connection.
        self.connection.memory()
    }
}

impl Memory for ConnectionZeroCounter {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.connection.reserve(min_bytes)
    }
}

// ###########################################################################
// Everything below this comment is dummy logic to make the example compile.
// The useful content of the example is the code above.
// ###########################################################################

#[derive(Debug)]
struct Connection;

impl Connection {
    const fn accept() -> Self {
        Self {}
    }

    #[expect(clippy::needless_pass_by_ref_mut, clippy::unused_self, reason = "for example realism")]
    fn write(&mut self, mut _message: BytesView) {}
}

impl HasMemory for Connection {
    fn memory(&self) -> impl MemoryShared {
        // This is a wrong way to implement this trait! Only to make the example compile.
        bytesbuf::mem::testing::TransparentMemory::new()
    }
}

impl Memory for Connection {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        // This is a wrong way to implement this trait! Only to make the example compile.
        bytesbuf::mem::testing::TransparentMemory::new().reserve(min_bytes)
    }
}
