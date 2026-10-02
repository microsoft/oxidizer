// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Formatter};
use std::fs::File;
use std::io;
#[cfg(all(not(target_os = "linux"), not(windows)))]
use std::process;

pub(super) enum StdioInner {
    Inherit,
    Null,
    Piped,
    File(File),
}

/// Configuration for a launched process's standard stream.
pub struct Stdio {
    pub(super) inner: StdioInner,
}

impl Stdio {
    /// Creates a pipe between the controller and launched process.
    #[must_use]
    pub const fn piped() -> Self {
        Self { inner: StdioInner::Piped }
    }

    /// Inherits the corresponding stream from the controller.
    #[must_use]
    pub const fn inherit() -> Self {
        Self {
            inner: StdioInner::Inherit,
        }
    }

    /// Connects the stream to the platform null device.
    #[must_use]
    pub const fn null() -> Self {
        Self { inner: StdioInner::Null }
    }

    pub(super) fn try_clone(&self) -> io::Result<Self> {
        let inner = match &self.inner {
            StdioInner::Inherit => StdioInner::Inherit,
            StdioInner::Null => StdioInner::Null,
            StdioInner::Piped => StdioInner::Piped,
            StdioInner::File(file) => StdioInner::File(file.try_clone()?),
        };
        Ok(Self { inner })
    }

    #[cfg(all(not(target_os = "linux"), not(windows)))]
    pub(super) fn into_std(self) -> process::Stdio {
        match self.inner {
            StdioInner::Inherit => process::Stdio::inherit(),
            StdioInner::Null => process::Stdio::null(),
            StdioInner::Piped => process::Stdio::piped(),
            StdioInner::File(file) => file.into(),
        }
    }
}

impl Debug for Stdio {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let kind = match self.inner {
            StdioInner::Inherit => "inherit",
            StdioInner::Null => "null",
            StdioInner::Piped => "piped",
            StdioInner::File(_) => "file",
        };
        f.debug_tuple("Stdio").field(&kind).finish()
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Self {
        Self {
            inner: StdioInner::File(file),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn clones_symbolic_stdio_forms() {
        assert!(matches!(Stdio::inherit().try_clone().unwrap().inner, StdioInner::Inherit));
        assert!(matches!(Stdio::null().try_clone().unwrap().inner, StdioInner::Null));
        assert!(matches!(Stdio::piped().try_clone().unwrap().inner, StdioInner::Piped));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn clones_file_backed_stdio() {
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(matches!(Stdio::from(file).try_clone().unwrap().inner, StdioInner::File(_)));
    }
}
