// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Result;
use std::sync::{Arc, Mutex};

use crate::dir_entry::DirEntry;
use crate::dispatcher::Dispatcher;

/// An asynchronous iterator over the entries in a directory.
///
/// This struct is returned from [`Directory::read_dir`](crate::directory::Directory::read_dir)
/// and will yield instances of [`DirEntry`](crate::dir_entry::DirEntry). Through a `DirEntry`,
/// information like the entry's file name and possibly other metadata can be learned.
///
/// The entries are fetched lazily from the underlying filesystem on each call to
/// [`next_entry`](ReadDir::next_entry), dispatching to a worker thread.
#[derive(Debug)]
pub struct ReadDir {
    inner: Arc<Mutex<std::fs::ReadDir>>,
    dispatcher: Dispatcher,
}

impl ReadDir {
    pub(crate) fn new(read_dir: std::fs::ReadDir, dispatcher: Dispatcher) -> Self {
        Self {
            inner: Arc::new(Mutex::new(read_dir)),
            dispatcher,
        }
    }

    /// Returns the next entry in the directory stream.
    ///
    /// Returns `Ok(None)` when there are no more entries.
    ///
    /// # Errors
    ///
    /// May return an I/O error if there was a problem reading a particular entry.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "&mut self enforces sequential cursor access across dispatch boundary"
    )]
    pub async fn next_entry(&mut self) -> Result<Option<DirEntry>> {
        let inner = Arc::clone(&self.inner);
        self.dispatcher
            .dispatch(move || {
                let mut guard = inner.lock().map_err(|e| std::io::Error::other(format!("dir lock poisoned: {e}")))?;
                match guard.next() {
                    Some(Ok(entry)) => Ok(Some(DirEntry::from_std(&entry))),
                    Some(Err(e)) => Err(e),
                    None => Ok(None),
                }
            })
            .await
    }
}
