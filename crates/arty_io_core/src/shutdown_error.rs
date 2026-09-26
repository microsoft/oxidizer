// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

/// An error returned when a driver cannot complete graceful shutdown.
#[derive(Debug)]
pub struct ShutdownError {
    kind: ShutdownErrorKind,
}

#[derive(Debug)]
enum ShutdownErrorKind {
    Message(Box<str>),
    Source(Box<dyn Error + Send + Sync + 'static>),
}

impl ShutdownError {
    /// Creates an error with the given message.
    #[must_use]
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            kind: ShutdownErrorKind::Message(message.into().into_boxed_str()),
        }
    }

    /// Creates an error with the given source.
    #[must_use]
    pub fn from_source(source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            kind: ShutdownErrorKind::Source(Box::new(source)),
        }
    }
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ShutdownErrorKind::Message(message) => f.write_str(message),
            ShutdownErrorKind::Source(_) => f.write_str("i/o driver shutdown failed"),
        }
    }
}

impl Error for ShutdownError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            ShutdownErrorKind::Message(_) => None,
            ShutdownErrorKind::Source(source) => Some(source.as_ref()),
        }
    }
}
