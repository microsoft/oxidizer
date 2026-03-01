// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Positional I/O — reading and writing at specific offsets.
//!
//! Positional methods like [`PositionalFile::read_at`] and
//! [`PositionalFile::write_at`] do not move the file cursor, enabling
//! concurrent access to different regions of the same file.

use file::{PositionalFile, Root};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // Seed a file with known content.
    dir.write_slice("pos.bin", b"AAAA____BBBB").await?;

    let pf = PositionalFile::open(&dir, "pos.bin").await?;

    // Overwrite the middle section without touching the rest.
    pf.write_slice_at(4, b"XXXX").await?;

    // Read back individual regions — no cursor is involved.
    let head = pf.read_exact_at(0, 4).await?;
    let mid = pf.read_exact_at(4, 4).await?;
    let tail = pf.read_exact_at(8, 4).await?;
    println!(
        "head={:?}  mid={:?}  tail={:?}",
        std::str::from_utf8(head.first_slice()),
        std::str::from_utf8(mid.first_slice()),
        std::str::from_utf8(tail.first_slice()),
    );

    Ok(())
}
