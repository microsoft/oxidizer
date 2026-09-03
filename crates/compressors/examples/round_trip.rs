// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compressing and decompressing a whole buffer.

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::format::Format;
use compressors::{Resources, Result, gzip};

fn main() -> Result<()> {
    // Every output buffer is allocated from this provider.
    let memory = GlobalPool::new();
    let original = b"the quick brown fox jumps over the lazy dog. ".repeat(64);

    let compressed = gzip::compress(BytesView::copied_from_slice(&original, &memory), &Resources::default())?;
    let decompressed = gzip::decompress(compressed.clone(), &Resources::default())?;

    assert_eq!(decompressed.to_vec(), original);
    println!("gzip: {} -> {} bytes", original.len(), compressed.len());

    // The same payload through a format chosen at run time.
    for &format in Format::ALL {
        let input = BytesView::copied_from_slice(&original, &memory);
        let compressed = compressors::format::compress(format, input, &Resources::default())?;
        let decompressed = compressors::format::decompress(format, compressed.clone(), &Resources::default())?;

        assert_eq!(decompressed.to_vec(), original);
        println!("{format:?}: {} bytes", compressed.len());
    }

    Ok(())
}
