// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed file access with [`ReadOnlyFile`], [`WriteOnlyFile`], and [`File`].
//!
//! Each type enforces its access level at compile time. A [`File`] can
//! be narrowed to either single-access type via [`From`] conversions.

use bytesbuf::mem::Memory;
use file::{File, ReadOnlyFile, Root, WriteOnlyFile};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // WriteOnlyFile — can write, cannot read.
    let mut wf = WriteOnlyFile::create(&dir, "data.txt").await?;
    let mut buf = wf.reserve(64);
    buf.put_slice(*b"written via WriteOnlyFile");
    wf.write(buf.consume_all()).await?;
    wf.flush().await?;
    println!("wrote via WriteOnlyFile");

    // ReadOnlyFile — can read, cannot write.
    let mut rf = ReadOnlyFile::open(&dir, "data.txt").await?;
    let view = rf.read_max(1024).await?;
    println!("read via ReadOnlyFile: {:?}", std::str::from_utf8(view.first_slice()));

    // File — full access.
    let rw = File::open(&dir, "data.txt").await?;
    let meta = rw.metadata().await?;
    println!("file length via File: {} bytes", meta.len());

    // Narrow a File down to ReadOnlyFile.
    let mut ro: ReadOnlyFile = rw.into();
    let view = ro.read_max(1024).await?;
    println!("after narrowing: {:?}", std::str::from_utf8(view.first_slice()));

    Ok(())
}
