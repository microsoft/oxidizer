// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Streaming I/O — reading and writing files in chunks.
//!
//! Shows how to write data incrementally with [`WriteOnlyFile`] and
//! then stream it back with [`ReadOnlyFile::read_max`].

use bytesbuf::mem::Memory;
use file::{ReadOnlyFile, Root, WriteOnlyFile};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // Write 10 chunks into a file.
    let mut wf = WriteOnlyFile::create(&dir, "stream.bin").await?;
    for i in 0u8..10 {
        let mut buf = wf.reserve(128);
        let line = format!("chunk {i}\n");
        buf.put_slice(line.as_bytes());
        wf.write(buf.consume_all()).await?;
    }
    wf.flush().await?;
    println!("wrote 10 chunks");

    // Stream the file back in small pieces.
    let mut rf = ReadOnlyFile::open(&dir, "stream.bin").await?;
    let mut total = 0usize;
    loop {
        let chunk = rf.read_max(16).await?;
        if chunk.is_empty() {
            break; // EOF
        }
        total += chunk.len();
        let text = std::str::from_utf8(chunk.first_slice()).unwrap_or("?");
        print!("{text}");
    }
    println!("---\nread {total} bytes total");

    Ok(())
}
