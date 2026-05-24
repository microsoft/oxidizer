// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]

//! Shows `#[thunk(from = me.thunker())]` — the thunker is reached via a
//! method call on an owned `Arc<Self>` parameter.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

struct FileSystem {
    inner_thunker: Thunker,
}

impl FileSystem {
    fn thunker(&self) -> &Thunker {
        &self.inner_thunker
    }

    async fn list_files(self: &Arc<Self>, dir: PathBuf) -> std::io::Result<Vec<String>> {
        Self::list_files_thunked(Arc::clone(self), dir).await
    }

    #[thunk(from = me.thunker())]
    async fn list_files_thunked(me: Arc<Self>, dir: PathBuf) -> std::io::Result<Vec<String>> {
        std::fs::read_dir(dir)?
            .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let fs = Arc::new(FileSystem {
        inner_thunker: Thunker::builder()
            .max_thread_count(2)
            .cool_down_interval(Duration::from_secs(5))
            .build(),
    });

    println!("calling thread: {:?}", std::thread::current().id());

    match fs.list_files(PathBuf::from(".")).await {
        Ok(files) => {
            println!("files in current directory:");
            for f in &files {
                println!("  {f}");
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
