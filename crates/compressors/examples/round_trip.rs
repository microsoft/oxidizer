// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compressing and decompressing a whole buffer.

use bytesbuf::mem::GlobalPool;
use compressors::format::Format;
use compressors::{Resources, Result, gzip};

fn main() -> Result<()> {
    // Held once and handed to every operation, so input views and output buffers both come from
    // this provider and every engine returns to the same pool when its compressor drops.
    let resources = Resources::new(GlobalPool::new());
    let original = b"the quick brown fox jumps over the lazy dog. ".repeat(64);

    let compressed = gzip::compress(&*original, &resources)?;
    let decompressed = gzip::decompress(compressed.clone(), &resources)?;

    assert_eq!(decompressed.to_vec(), original);
    println!("gzip: {} -> {} bytes", original.len(), compressed.len());

    // The same payload through a format chosen at run time.
    for &format in Format::ALL {
        let compressed = compressors::format::compress(format, &*original, &resources)?;
        let decompressed = compressors::format::decompress(format, compressed.clone(), &resources)?;

        assert_eq!(decompressed.to_vec(), original);
        println!("{format:?}: {} bytes", compressed.len());
    }

    Ok(())
}
