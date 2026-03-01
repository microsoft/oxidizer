// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{Error, ErrorKind, Result};
use std::path::Path;

use crate::directory::Directory;
use crate::dispatcher::Dispatcher;

/// The entry point for capability-based filesystem access.
///
/// `Root` provides the sole mechanism for obtaining a [`Directory`] capability.
/// Once a directory is bound, all filesystem operations are scoped to that
/// directory and its descendants.
#[derive(Debug)]
pub struct Root;

impl Root {
    /// Binds to a directory on the filesystem, returning a [`Directory`] capability.
    ///
    /// The given path is the only point where an absolute or arbitrary path is accepted.
    /// All subsequent filesystem operations through the returned `Directory` use
    /// paths relative to this root.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist, is not a directory, or if
    /// the process lacks permission to access it.
    pub async fn bind(path: impl AsRef<Path>) -> Result<Directory> {
        let path = path.as_ref().to_path_buf();
        let dispatcher = Dispatcher::new();
        let base_path = dispatcher
            .dispatch(move || {
                let canonical = std::fs::canonicalize(&path)?;
                let metadata = std::fs::metadata(&canonical)?;
                if !metadata.is_dir() {
                    return Err(Error::new(ErrorKind::NotADirectory, "path is not a directory"));
                }
                Ok(canonical)
            })
            .await?;
        Ok(Directory::new(base_path, dispatcher))
    }
}
