// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows `#[thunk(from = self.thunker)]` — the thunker is a struct field.
//!
//! This is the most common pattern: a struct holds a `Thunker` and its
//! methods use it for dispatch.

use std::path::Path;
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

struct FileSystem {
    thunker: Thunker,
}

impl FileSystem {
    /// Lists files in a directory — dispatched to a worker thread.
    #[thunk(from = self.thunker)]
    async fn list_files(&self, dir: &Path) -> std::io::Result<Vec<String>> {
        std::fs::read_dir(dir)?
            .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let fs = FileSystem {
        thunker: Thunker::builder()
            .max_thread_count(2)
            .cool_down_interval(Duration::from_secs(5))
            .build(),
    };

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
