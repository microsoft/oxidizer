// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::error::Error;
use core::fmt;

use super::JsonError;

/// An error from fallible per-element JSON deserialization.
///
/// This preserves callback failures separately from JSON, allocation, and
/// resource-limit failures.
#[derive(Debug)]
#[non_exhaustive]
pub enum JsonEachError<E> {
    /// JSON deserialization, allocation, or resource-limit failure.
    Json(JsonError),
    /// Error returned by the element callback.
    Callback(E),
}

impl<E> JsonEachError<E> {
    /// Returns the JSON error, if deserialization failed.
    #[must_use]
    pub const fn json_error(&self) -> Option<&JsonError> {
        match self {
            Self::Json(error) => Some(error),
            Self::Callback(_) => None,
        }
    }

    /// Returns the callback error, if the callback failed.
    #[must_use]
    pub const fn callback_error(&self) -> Option<&E> {
        match self {
            Self::Json(_) => None,
            Self::Callback(error) => Some(error),
        }
    }

    /// Consumes this error and returns the callback error, if present.
    pub fn into_callback_error(self) -> Option<E> {
        match self {
            Self::Json(_) => None,
            Self::Callback(error) => Some(error),
        }
    }
}

impl<E> From<JsonError> for JsonEachError<E> {
    fn from(error: JsonError) -> Self {
        Self::Json(error)
    }
}

impl<E> fmt::Display for JsonEachError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::Callback(_) => f.write_str("JSON element callback failed"),
        }
    }
}

impl<E: Error + 'static> Error for JsonEachError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Callback(error) => Some(error),
        }
    }
}
