// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Flexible file creation with [`OpenOptions`].
//!
//! [`OpenOptions`] provides a builder for controlling exactly how a file
//! is opened — analogous to [`std::fs::OpenOptions`] but fully async and
//! capability-scoped.

use file::{OpenOptions, Root};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // Create a new file for reading and writing.
    let mut rw = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&dir, "options.txt")
        .await?;

    rw.write_slice(b"line 1\n").await?;
    rw.write_slice(b"line 2\n").await?;
    rw.flush().await?;

    // Rewind and read everything back.
    rw.rewind().await?;
    let data = rw.read_max(4096).await?;
    let text = std::str::from_utf8(data.first_slice()).unwrap_or("?");
    println!("initial content:\n{text}");

    // Re-open in truncate mode to clear the file.
    let mut rw = OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(true)
        .open(&dir, "options.txt")
        .await?;

    rw.write_slice(b"fresh start\n").await?;
    rw.flush().await?;
    rw.rewind().await?;
    let data = rw.read_max(4096).await?;
    let text = std::str::from_utf8(data.first_slice()).unwrap_or("?");
    println!("after truncate:\n{text}");

    // Open in append mode — writes always go to the end.
    let mut appender = OpenOptions::new().append(true).open(&dir, "options.txt").await?;

    appender.write_slice(b"appended line\n").await?;
    appender.flush().await?;

    let final_text = dir.read_to_string("options.txt").await?;
    println!("final content:\n{final_text}");

    Ok(())
}
