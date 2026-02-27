// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{Error, ErrorKind, Result};
use std::path::Path;

use sync_thunk::{Thunker, thunk};

use crate::directory::Directory;

/// The entry point for capability-based filesystem access.
///
/// `Root` provides the sole mechanism for obtaining a [`Directory`] capability.
/// Once a directory is bound, all filesystem operations are scoped to that
/// directory and its descendants.
#[derive(Debug)]
pub struct Root;

#[expect(missing_docs, reason = "thunk macro generates a blocking helper without its own doc comment")]
impl Root {
    /// Binds to a directory on the filesystem, returning a [`Directory`] capability.
    ///
    /// The given path is the only point where an absolute or arbitrary path is accepted.
    /// All subsequent filesystem operations through the returned `Directory` use
    /// paths relative to this root.
    ///
    /// The provided [`Thunker`] will be used for all blocking I/O dispatched by
    /// the returned directory and any files opened through it.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist, is not a directory, or if
    /// the process lacks permission to access it.
    #[thunk(from = thunker)]
    pub async fn bind_std(thunker: &Thunker, path: &Path) -> Result<Directory> {
        let canonical = std::fs::canonicalize(path)?;
        let metadata = std::fs::metadata(&canonical)?;
        if !metadata.is_dir() {
            return Err(Error::new(ErrorKind::NotADirectory, "path is not a directory"));
        }
        Ok(Directory::new(canonical, thunker.clone()))
    }
}
