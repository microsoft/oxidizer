// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::build_error_entry::BuildErrorEntry;

/// An error caused by invalid resolver configuration.
///
/// Returned when an HTTP method token is invalid or when a generated resolver
/// builder finds invalid or missing dynamic route registrations.
///
/// # Examples
///
/// ```
/// use routerama::HttpMethod;
///
/// let error = HttpMethod::custom("BAD METHOD").expect_err("spaces are not allowed");
/// assert_eq!(error.invalid_http_method_value(), Some("BAD METHOD"));
/// ```
#[derive(Debug)]
pub struct ConfigurationError {
    kind: ConfigurationErrorKind,
}

#[derive(Debug)]
enum ConfigurationErrorKind {
    InvalidHttpMethod(String),
    Resolver(Vec<BuildErrorEntry>),
}

impl ConfigurationError {
    pub(crate) fn invalid_http_method(value: String) -> Self {
        Self {
            kind: ConfigurationErrorKind::InvalidHttpMethod(value),
        }
    }

    pub(crate) fn resolver(entries: Vec<BuildErrorEntry>) -> Self {
        Self {
            kind: ConfigurationErrorKind::Resolver(entries),
        }
    }

    /// Returns the rejected HTTP method, when method validation failed.
    #[must_use]
    pub fn invalid_http_method_value(&self) -> Option<&str> {
        match &self.kind {
            ConfigurationErrorKind::InvalidHttpMethod(value) => Some(value),
            ConfigurationErrorKind::Resolver(_) => None,
        }
    }

    /// Iterates the upstream errors retained from failed route registrations.
    ///
    /// Resolver construction may aggregate multiple failures, so this supplements
    /// [`core::error::Error::source`], which returns only the first cause.
    pub fn causes(&self) -> impl Iterator<Item = &(dyn core::error::Error + 'static)> {
        let entries = match &self.kind {
            ConfigurationErrorKind::InvalidHttpMethod(_) => &[][..],
            ConfigurationErrorKind::Resolver(entries) => entries.as_slice(),
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
            ConfigurationErrorKind::Resolver(entries) => {
                f.write_str("failed to build resolver:")?;
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
