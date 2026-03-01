// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic file read and write operations.
//!
//! Demonstrates binding to a directory with [`Root::bind`] and using
//! the [`Directory`] capability to read and write files.

use file::Root;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // Write a file from a byte slice.
    dir.write_slice("greeting.txt", b"Hello, world!").await?;

    // Read the entire file back as bytes.
    let contents = dir.read("greeting.txt").await?;
    println!("read {} bytes: {:?}", contents.len(), contents.first_slice());

    // Read the file as a UTF-8 string.
    let text = dir.read_to_string("greeting.txt").await?;
    println!("text: {text}");

    // Overwrite with new content.
    dir.write_slice("greeting.txt", b"Goodbye!").await?;
    let updated = dir.read_to_string("greeting.txt").await?;
    println!("updated: {updated}");

    Ok(())
}
