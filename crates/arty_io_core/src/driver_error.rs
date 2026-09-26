// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

/// A failure to initialize, host, or service an I/O driver.
///
/// An individual failed I/O operation belongs in that operation's result, not here.
#[derive(Debug)]
pub struct DriverError {
    kind: DriverErrorKind,
}

#[derive(Debug)]
enum DriverErrorKind {
    Message(Box<str>),
    Source(Box<dyn Error + Send + Sync + 'static>),
}

impl DriverError {
    /// Creates an error with the given message.
    #[must_use]
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            kind: DriverErrorKind::Message(message.into().into_boxed_str()),
        }
    }

    /// Creates an error with the given source.
    #[must_use]
    pub fn from_source(source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            kind: DriverErrorKind::Source(Box::new(source)),
        }
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            DriverErrorKind::Message(message) => f.write_str(message),
            DriverErrorKind::Source(_) => f.write_str("i/o driver failed"),
        }
    }
}

impl Error for DriverError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            DriverErrorKind::Message(_) => None,
            DriverErrorKind::Source(source) => Some(source.as_ref()),
        }
    }
}
