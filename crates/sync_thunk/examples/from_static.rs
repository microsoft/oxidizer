// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]

//! Shows `#[thunk(from = THUNKER)]` — the thunker is a global static.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

static THUNKER: LazyLock<Thunker> = LazyLock::new(|| {
    Thunker::builder()
        .max_thread_count(2)
        .cool_down_interval(Duration::from_secs(5))
        .build()
});

struct FileSystem;

impl FileSystem {
    #[thunk(from = THUNKER)]
    async fn list_files(dir: PathBuf) -> std::io::Result<Vec<String>> {
        std::fs::read_dir(dir)?
            .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect()
    }
}

#[tokio::main]
async fn main() {
    println!("calling thread: {:?}", std::thread::current().id());

    match FileSystem::list_files(PathBuf::from(".")).await {
        Ok(files) => {
            println!("files in current directory:");
            for f in &files {
                println!("  {f}");
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
