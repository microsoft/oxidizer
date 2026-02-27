// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows `#[thunk(from = thunker)]` — the thunker is a function parameter.
//!
//! This is useful for associated functions (no `self` receiver), free functions,
//! or bootstrap code where a thunker is passed in by the caller.

use std::path::Path;
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

struct FileSystem;

impl FileSystem {
    /// Lists files in a directory — dispatched to a worker thread.
    ///
    /// The thunker is passed as a parameter because this is an associated
    /// function with no `self` receiver.
    #[thunk(from = thunker)]
    async fn list_files(thunker: &Thunker, dir: &Path) -> std::io::Result<Vec<String>> {
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

    match FileSystem::list_files(&thunker, Path::new(".")).await {
        Ok(files) => {
            println!("files in current directory:");
            for f in &files {
                println!("  {f}");
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
