// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows `#[thunk(from = THUNKER)]` — the thunker is a global static.
//!
//! This is useful for applications that share a single thread pool across
//! many components without threading a `Thunker` through every struct.

use std::path::Path;
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
    /// Lists files in a directory — dispatched to a worker thread.
    #[thunk(from = THUNKER)]
    async fn list_files(&self, dir: &Path) -> std::io::Result<Vec<String>> {
        std::fs::read_dir(dir)?
            .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let fs = FileSystem;

    println!("calling thread: {:?}", std::thread::current().id());

    match fs.list_files(Path::new(".")).await {
        Ok(files) => {
            println!("files in current directory:");
            for f in &files {
                println!("  {f}");
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
