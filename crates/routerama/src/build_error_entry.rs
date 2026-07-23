// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::String;
use core::fmt;

use http_path_template::ParseError;

/// A problem found while building a resolver.
#[derive(Debug)]
pub(crate) enum BuildErrorEntry {
    /// A dynamic variant's registration method was never called.
    MissingRoute { add_method: &'static str, variant: &'static str },
    /// A registered path failed to parse as a path template.
    InvalidTemplate {
        variant: &'static str,
        path: String,
        source: ParseError,
    },
    /// A registered path's captures do not match the variant's declared fields.
    CaptureMismatch {
        variant: &'static str,
        path: String,
        expected: String,
        found: String,
    },
    /// Two dynamic registrations match the same requests.
    Conflict { message: String },
}

impl BuildErrorEntry {
    pub(crate) fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::InvalidTemplate { source, .. } => Some(source),
            Self::MissingRoute { .. } | Self::CaptureMismatch { .. } | Self::Conflict { .. } => None,
        }
    }
}

impl fmt::Display for BuildErrorEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoute { add_method, variant } => write!(
                f,
                "`{add_method}` was never called, but it is required to initialize the dynamic `{variant}` route",
            ),
            Self::InvalidTemplate { variant, path, source } => write!(
                f,
                "the dynamic `{variant}` route was registered with an invalid path `{path}`: {source}",
            ),
            Self::CaptureMismatch {
                variant,
                path,
                expected,
                found,
            } => write!(
                f,
                "the dynamic `{variant}` route was registered with path `{path}` whose captures {found} do not match its fields {expected}",
            ),
            Self::Conflict { message } => f.write_str(message),
        }
    }
}
