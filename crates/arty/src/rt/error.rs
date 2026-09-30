// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;
use std::fmt::{self, Display};

use crate::rt::config::BuildError;

/// A specialized `Result` type for Arty runtime operations.
pub type Result<T> = std::result::Result<T, Error>;

/// An opaque error originating in the Arty runtime.
///
/// Use [`Display`] and [`StdError::source`] for diagnostics.
/// Error representations and diagnostic text are not recovery classifications.
/// Runtime construction failures convert from [`BuildError`] without losing their cause.
#[derive(Debug)]
pub struct Error {
    source: Box<dyn StdError + Send + Sync>,
}

impl Error {
    pub(crate) fn new(source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self { source: source.into() }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.source.as_ref(), f)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

impl From<BuildError> for Error {
    fn from(source: BuildError) -> Self {
        Self::new(source)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn internal_constructor_preserves_display_context() {
        let marker = "I/O context marker";
        let error = Error::new(io::Error::other(marker));
        assert!(error.to_string().contains(marker));
    }

    #[test]
    fn internal_constructor_preserves_typed_source() {
        let error = Error::new(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(
            error.source().unwrap().downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied,
        );
    }

    #[test]
    fn internal_constructor_preserves_boxed_source() {
        let source: Box<dyn StdError + Send + Sync> = Box::new(io::Error::from(io::ErrorKind::TimedOut));
        let error = Error::new(source);
        assert_eq!(
            error.source().unwrap().downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut,
        );
    }

    #[test]
    fn build_conversion_preserves_typed_source() {
        let error = Error::from(BuildError::insufficient_processors(2, 1));
        assert!(error.source().unwrap().is::<BuildError>());
    }
}
