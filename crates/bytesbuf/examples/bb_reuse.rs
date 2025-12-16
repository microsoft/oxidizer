// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how you can easily and cheaply reuse BytesView in part or whole.

use bytesbuf::{BytesView, GlobalPool};

fn main() {
    // The global memory pool in real-world code would be provided by the application framework.
    let memory = GlobalPool::new();

    let hello_world = BytesView::copied_from_slice(b"Hello, world!", &memory);

    inspect_bytes(&hello_world);

    // Splitting up a view into sub-views is a cheap zero-copy operation.
    let hello = hello_world.range(0..5);
    let world = hello_world.range(7..12);

    inspect_bytes(&hello);
    inspect_bytes(&world);

    // You can glue the parts back together if you wish. Again, this is a cheap zero-copy operation.
    let hello_world_reconstructed = BytesView::from_views([hello, world]);

    inspect_bytes(&hello_world_reconstructed);

    // You can also append a BytesView into a BytesBuf. This is also a cheap zero-copy operation.
    let mut buf = memory.reserve(1024);

    buf.put_slice(b"The quick brown fox says \"".as_slice());
    buf.put_bytes(hello_world_reconstructed);
    buf.put_slice(b"\" and jumps over the lazy dog.".as_slice());

    let fox_story = buf.consume_all();

    inspect_bytes(&fox_story);
}

fn inspect_bytes(bytes: &BytesView) {
    let len = bytes.len();
    let mut slice_lengths = Vec::new();

    // We need to mutate the view to slide our inspection window over it, so we clone it.
    // Cloning a view is a cheap zero-copy operation; do not hesitate to do it when needed.
    let mut bytes = bytes.clone();

    while !bytes.is_empty() {
        let slice = bytes.first_slice();
        slice_lengths.push(slice.len());

        // We have completed processing this slice. All we wanted was to know its length.
        // We can now mark this slice as consumed, revealing the next slice for inspection.
        bytes.advance(slice.len());
    }

    println!("Inspected a view over {len} bytes with slice lengths: {slice_lengths:?}");
}
