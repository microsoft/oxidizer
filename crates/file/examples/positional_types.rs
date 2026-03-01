// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Positional file types — [`ReadOnlyPositionalFile`], [`WriteOnlyPositionalFile`],
//! and [`PositionalFile`].
//!
//! Positional files have no cursor. Every I/O operation specifies an explicit
//! byte offset and takes `&self`, enabling concurrent access from multiple tasks.

use file::{PositionalFile, ReadOnlyPositionalFile, Root, WriteOnlyPositionalFile};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // WriteOnlyPositionalFile — write-only access at explicit offsets.
    let wf = WriteOnlyPositionalFile::create(&dir, "pos.txt").await?;
    wf.write_slice_at(0, b"Hello, ").await?;
    wf.write_slice_at(7, b"positional world!").await?;
    wf.flush().await?;
    println!("wrote via WriteOnlyPositionalFile");

    // ReadOnlyPositionalFile — read-only access at explicit offsets.
    let rf = ReadOnlyPositionalFile::open(&dir, "pos.txt").await?;
    let view = rf.read_exact_at(0, 24).await?;
    println!("read via ReadOnlyPositionalFile: {:?}", std::str::from_utf8(view.first_slice()),);
    // Read a sub-range without affecting any cursor.
    let mid = rf.read_exact_at(7, 10).await?;
    println!("  sub-range [7..17]: {:?}", std::str::from_utf8(mid.first_slice()));

    // PositionalFile — full read-write access.
    let pf = PositionalFile::open(&dir, "pos.txt").await?;
    // Overwrite part of the file and then read back.
    pf.write_slice_at(7, b"POSITIONAL WORLD!").await?;
    let view = pf.read_exact_at(0, 24).await?;
    println!("read via PositionalFile: {:?}", std::str::from_utf8(view.first_slice()),);

    // Narrow a PositionalFile down to ReadOnlyPositionalFile.
    let ro: ReadOnlyPositionalFile = pf.into();
    let view = ro.read_exact_at(0, 24).await?;
    println!("after narrowing: {:?}", std::str::from_utf8(view.first_slice()),);

    // Concurrent reads — positional I/O takes &self, so multiple
    // reads can run in parallel without cursor conflicts.
    let (a, b) = tokio::join!(ro.read_exact_at(0, 5), ro.read_exact_at(7, 10));
    println!(
        "concurrent reads: {:?} and {:?}",
        std::str::from_utf8(a?.first_slice()),
        std::str::from_utf8(b?.first_slice()),
    );

    Ok(())
}
