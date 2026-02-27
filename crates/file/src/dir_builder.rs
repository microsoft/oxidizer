// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Result;
use std::path::Path;

use crate::directory::Directory;
use crate::path_utils::safe_join;

/// A builder used to create directories in various manners.
///
/// This builder allows configuring whether directories should be created
/// recursively.
#[derive(Debug)]
pub struct DirBuilder {
    recursive: bool,
}

impl DirBuilder {
    /// Creates a new set of options with default mode and recursive set to `false`.
    #[must_use]
    pub const fn new() -> Self {
        Self { recursive: false }
    }

    /// Indicates that directories should be created recursively, creating all
    /// parent components if they are missing.
    ///
    /// When set to `false` (the default), only a single directory level is
    /// created.
    pub const fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    /// Creates the specified directory with the options configured in this builder.
    ///
    /// The path is relative to the given directory capability.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory already exists (when not recursive),
    /// if the parent does not exist (when not recursive), or if the process
    /// lacks permissions.
    pub async fn create(&self, dir: &Directory, path: impl AsRef<Path>) -> Result<()> {
        let full_path = safe_join(dir.base_path(), path)?;
        let recursive = self.recursive;
        dir.dispatcher()
            .dispatch(move || {
                if recursive {
                    std::fs::create_dir_all(&full_path)
                } else {
                    std::fs::create_dir(&full_path)
                }
            })
            .await
    }
}

impl Default for DirBuilder {
    fn default() -> Self {
        Self::new()
    }
}
