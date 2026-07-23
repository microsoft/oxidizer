// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::build_error_entry::BuildErrorEntry;

/// An error caused by invalid runtime route configuration.
///
/// Returned when an HTTP method token is invalid, when a generated resolver
/// builder finds invalid or missing dynamic route registrations, or when an
/// erased mount router contains invalid or conflicting registrations.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "resolve")]
/// # fn main() {
/// use routerama::resolve::HttpMethod;
///
/// let error = HttpMethod::custom("BAD METHOD").expect_err("spaces are not allowed");
/// assert_eq!(error.invalid_http_method_value(), Some("BAD METHOD"));
/// # }
/// # #[cfg(not(feature = "resolve"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ConfigurationError {
    kind: ConfigurationErrorKind,
}

#[derive(Debug)]
enum ConfigurationErrorKind {
    InvalidHttpMethod(String),
    Routes {
        context: &'static str,
        entries: Vec<BuildErrorEntry>,
    },
}

impl ConfigurationError {
    pub(crate) fn invalid_http_method(value: String) -> Self {
        Self {
            kind: ConfigurationErrorKind::InvalidHttpMethod(value),
        }
    }

    pub(crate) fn resolver(entries: Vec<BuildErrorEntry>) -> Self {
        Self {
            kind: ConfigurationErrorKind::Routes {
                context: "resolver",
                entries,
            },
        }
    }

    #[cfg(feature = "mount")]
    pub(crate) fn mounts(entries: Vec<BuildErrorEntry>) -> Self {
        Self {
            kind: ConfigurationErrorKind::Routes {
                context: "erased mount router",
                entries,
            },
        }
    }

    /// Returns the rejected HTTP method, when method validation failed.
    #[must_use]
    pub fn invalid_http_method_value(&self) -> Option<&str> {
        match &self.kind {
            ConfigurationErrorKind::InvalidHttpMethod(value) => Some(value),
            ConfigurationErrorKind::Routes { .. } => None,
        }
    }

    /// Iterates the upstream errors retained from failed route registrations.
    ///
    /// Route construction may aggregate multiple failures, so this supplements
    /// [`core::error::Error::source`], which returns only the first cause.
    pub fn causes(&self) -> impl Iterator<Item = &(dyn core::error::Error + 'static)> {
        let entries = match &self.kind {
            ConfigurationErrorKind::InvalidHttpMethod(_) => &[][..],
            ConfigurationErrorKind::Routes { entries, .. } => entries.as_slice(),
        };
        entries.iter().filter_map(BuildErrorEntry::source)
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ConfigurationErrorKind::InvalidHttpMethod(value) => {
                write!(f, "`{value}` is not a valid RFC 9110 HTTP method token")
            }
            ConfigurationErrorKind::Routes { context, entries } => {
                write!(f, "failed to build {context}:")?;
                for entry in entries {
                    write!(f, "\n  - {entry}")?;
                }
                Ok(())
            }
        }
    }
}

impl core::error::Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        self.causes().next()
    }
}
