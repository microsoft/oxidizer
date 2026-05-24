// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]

//! Shows `#[thunk(from = thunker)]` — the thunker is itself a function
//! parameter. Useful when no `Self` is involved.

use std::path::PathBuf;
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

struct FileSystem;

impl FileSystem {
    #[thunk(from = thunker)]
    async fn list_files(thunker: Thunker, dir: PathBuf) -> std::io::Result<Vec<String>> {
        std::fs::read_dir(dir)?
            .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let thunker = Thunker::builder()
        .max_thread_count(2)
        .cool_down_interval(Duration::from_secs(5))
        .build();

    println!("calling thread: {:?}", std::thread::current().id());

    match FileSystem::list_files(thunker, PathBuf::from(".")).await {
        Ok(files) => {
            println!("files in current directory:");
            for f in &files {
                println!("  {f}");
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
