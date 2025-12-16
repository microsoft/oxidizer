// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to efficiently work with static data known at compile time.
use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use bytesbuf::{BytesView, HasMemory, Memory, MemoryShared, TransparentTestMemory};

// We often want to write static fragments as part of network communications.
const HEADER_PREFIX: &[u8] = b"Unix-Milliseconds: ";
const TWO_NEWLINES: &[u8] = b"\r\n\r\n";

fn main() {
    // We transform the static data into a BytesView on first use, via OnceLock.
    let header_prefix = OnceLock::<BytesView>::new();
    let two_newlines = OnceLock::<BytesView>::new();

    // Accept some connections and send a response to each of them.
    for _ in 0..10 {
        let mut connection = Connection::accept();

        // The static data is transformed into a BytesView on first use, using memory optimally configured
        // for network connections. The underlying principle is that memory optimally configured for one network
        // connection is likely also optimally configured for another network connection, enabling efficient reuse.
        let header_prefix = header_prefix.get_or_init(|| BytesView::copied_from_slice(HEADER_PREFIX, &connection.memory()));
        let two_newlines = two_newlines.get_or_init(|| BytesView::copied_from_slice(TWO_NEWLINES, &connection.memory()));

        // Note that reused BytesViews do not consume any memory capacity, so when making a reservation
        // for the response message, we only need to account for the timestamp bytes.
        let mut response_buf = connection.memory().reserve(TIMESTAMP_MAX_LEN);

        // Insert the static prefix. Cloning a BytesView is a cheap zero-copy operation.
        response_buf.put_bytes(header_prefix.clone());

        // We cannot assume that a `BytesBuf` contains consecutive memory,
        // so any fixed-length processing must be done using temporary buffers.
        let mut stringification_buffer = [0u8; TIMESTAMP_MAX_LEN];
        let timestamp_bytes = serialize_timestamp(&mut stringification_buffer);

        response_buf.put_slice(timestamp_bytes);

        // Insert the static suffix. Cloning a BytesView is a cheap zero-copy operation.
        response_buf.put_bytes(two_newlines.clone());

        connection.write(response_buf.consume_all());
    }
}

// Enough to hold any u128 as a string.
const TIMESTAMP_MAX_LEN: usize = 39;

fn serialize_timestamp(buffer: &mut [u8; TIMESTAMP_MAX_LEN]) -> &[u8] {
    let now = SystemTime::now();
    let unix_millis = now
        .duration_since(UNIX_EPOCH)
        .expect("impossible for time to be before unix epoch")
        .as_millis();

    let mut cursor = buffer.as_mut_slice();
    write!(cursor, "{unix_millis}").expect("buffer size is known good constant - u128 must fit");

    // cursor now contains the remaining bytes after writing the timestamp.
    let bytes_written = TIMESTAMP_MAX_LEN - cursor.len();

    &buffer[..bytes_written]
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
    fn write(&mut self, mut message: BytesView) {
        print!("Sent message: ");

        while !message.is_empty() {
            let slice = message.first_slice();
            print!("{}", String::from_utf8_lossy(slice));
            message.advance(slice.len());
        }

        println!();
    }
}

impl HasMemory for Connection {
    fn memory(&self) -> impl MemoryShared {
        // This is a wrong way to implement this trait! Only to make the example compile.
        TransparentTestMemory::new()
    }
}
