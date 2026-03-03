// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Result;

use sync_thunk::{Thunker, thunk};

use crate::dir_entry::DirEntry;

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
    inner: std::fs::ReadDir,
    thunker: Thunker,
}

impl ReadDir {
    pub(crate) fn new(read_dir: std::fs::ReadDir, thunker: Thunker) -> Self {
        Self { inner: read_dir, thunker }
    }

    /// Returns the next entry in the directory stream.
    ///
    /// Returns `Ok(None)` when there are no more entries.
    ///
    /// # Errors
    ///
    /// May return an I/O error if there was a problem reading a particular entry.
    #[inline]
    pub async fn next_entry(&mut self) -> Result<Option<DirEntry>> {
        self.next_entry_impl().await
    }

    #[thunk(from = self.thunker)]
    async fn next_entry_impl(&mut self) -> Result<Option<DirEntry>> {
        match self.inner.next() {
            Some(Ok(entry)) => Ok(Some(DirEntry::from_std(&entry))),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}
