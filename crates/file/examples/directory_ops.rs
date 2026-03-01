// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Directory operations — creating, listing, copying, and removing entries.
//!
//! Demonstrates the capability-scoped directory API including subdirectory
//! navigation, file copying across directories, and recursive removal.

use file::Root;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = Root::bind(tmp.path()).await?;

    // Create a nested directory structure.
    dir.create_dir_all("src/utils").await?;
    dir.create_dir("docs").await?;
    println!("created directories");

    // Populate some files.
    dir.write_slice("src/main.rs", b"fn main() {}").await?;
    dir.write_slice("src/utils/helpers.rs", b"// helpers").await?;
    dir.write_slice("docs/README.md", b"# Docs").await?;

    // List entries in the root.
    let mut entries = dir.read_dir(".").await?;
    println!("\nroot entries:");
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().map_err(|e| std::io::Error::other(e.to_string()))?;
        let kind = if file_type.is_dir() { "dir" } else { "file" };
        println!("  [{kind}] {}", entry.file_name().to_string_lossy());
    }

    // Navigate into a subdirectory.
    let src = dir.open_dir("src").await?;
    let mut src_entries = src.read_dir(".").await?;
    println!("\nsrc/ entries:");
    while let Some(entry) = src_entries.next_entry().await? {
        println!("  {}", entry.file_name().to_string_lossy());
    }

    // Copy a file across directories.
    let docs = dir.open_dir("docs").await?;
    dir.copy("src/main.rs", &docs, "main_backup.rs").await?;
    let backup = docs.read_to_string("main_backup.rs").await?;
    println!("\ncopied file contents: {backup}");

    // Rename a file within a directory.
    dir.rename("docs/README.md", &docs, "INDEX.md").await?;
    println!("renamed README.md -> INDEX.md");

    // Remove a single file and then an entire directory tree.
    dir.remove_file("src/utils/helpers.rs").await?;
    dir.remove_dir("src/utils").await?;
    dir.remove_dir_all("docs").await?;
    println!("cleaned up files and directories");

    Ok(())
}
