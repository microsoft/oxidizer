// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

/// A failure of driver initialization, completion service, or graceful shutdown.
///
/// Inspect the classification methods when deciding whether to select another configured native
/// arrangement or report a shutdown timeout. Messages provide context, not classification.
/// Native failures retain their underlying error through [`Error::source`].
/// A classification does not establish whether retrying after native side effects is safe;
/// the caller still applies the relevant rollback and recovery policy.
#[derive(Debug)]
pub struct DriverError {
    kind: ErrorKind,
    message: Box<str>,
    cause: Option<Box<dyn Error + Send + Sync + 'static>>,
}

#[derive(Debug, Eq, PartialEq)]
enum ErrorKind {
    Failure,
    Unsupported,
    DuplicateService,
    ShutdownTimeout,
}

impl DriverError {
    /// Creates a driver failure from a descriptive message.
    #[must_use]
    pub fn from_message(message: impl Into<String>) -> Self {
        Self::message(ErrorKind::Failure, message)
    }

    /// Creates a driver failure retaining its underlying cause.
    #[must_use]
    pub fn from_cause(cause: impl Error + Send + Sync + 'static) -> Self {
        Self::from_message(cause.to_string()).with_cause(cause)
    }

    /// Attaches an underlying cause without changing the classification or message.
    ///
    /// This allows an unsupported configuration or shutdown timeout to retain a native error
    /// and its source chain. Calling this again replaces the previous cause.
    #[must_use]
    pub fn with_cause(mut self, cause: impl Error + Send + Sync + 'static) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    /// Reports that this worker's native configuration cannot support the driver.
    ///
    /// This permits an explicit policy decision to try another configured arrangement. It does not
    /// authorize silently starting helper threads or changing native ownership.
    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::message(ErrorKind::Unsupported, message)
    }

    /// Reports expiration of the overall graceful-shutdown deadline.
    ///
    /// Timeout never authorizes releasing state that native operations or callbacks can still use.
    #[must_use]
    pub fn shutdown_timeout() -> Self {
        Self::message(ErrorKind::ShutdownTimeout, "i/o driver shutdown deadline expired")
    }

    /// Returns whether the worker's native configuration is unsupported.
    #[must_use]
    pub fn is_unsupported(&self) -> bool {
        self.kind == ErrorKind::Unsupported
    }

    /// Returns whether insertion conflicted with an already supplied client type.
    #[must_use]
    pub fn is_duplicate_completion_service(&self) -> bool {
        self.kind == ErrorKind::DuplicateService
    }

    /// Returns whether the overall graceful-shutdown deadline expired.
    #[must_use]
    pub fn is_shutdown_timeout(&self) -> bool {
        self.kind == ErrorKind::ShutdownTimeout
    }

    pub(crate) fn missing_service(name: &str) -> Self {
        Self::unsupported(format!("completion service {name} is unavailable"))
    }

    pub(crate) fn duplicate_service(name: &str) -> Self {
        Self::message(
            ErrorKind::DuplicateService,
            format!("completion service {name} was supplied more than once"),
        )
    }

    fn message(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            cause: None,
        }
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DriverError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.cause {
            Some(cause) => Some(cause.as_ref()),
            None => None,
        }
    }
}
