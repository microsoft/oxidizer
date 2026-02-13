// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::panic::Location;

use crate::{Enrichable, EnrichmentEntry};

use super::AppError;

/// Transforms [`Result`] and [`Option`] types into [`AppError`] with additional message.
///
/// For converting an error into [`AppError`] without additional context, use the `?` operator directly.
#[expect(clippy::missing_errors_doc, reason = "documentation for errors is not required here")]
pub trait IntoAppError<T> {
    /// Adds context message to the error.
    ///
    /// The message is converted to string only if the result is an error.
    fn into_app_err(self, msg: impl Display) -> Result<T, AppError>;

    /// Adds context message to the error.
    ///
    /// The function `msg_fn` is only called if the result is an error.
    fn into_app_err_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display;
}

impl<T, E> IntoAppError<T> for Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[track_caller]
    fn into_app_err(self, msg: impl Display) -> Result<T, AppError> {
        match self {
            Ok(value) => Ok(value),
            Err(e) => {
                let caller_location = Location::caller();
                let mut app_err = AppError::new(e);
                app_err.add_enrichment(EnrichmentEntry::new(
                    msg.to_string(),
                    caller_location.file(),
                    caller_location.line(),
                ));
                Err(app_err)
            }
        }
    }

    #[track_caller]
    fn into_app_err_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        match self {
            Ok(value) => Ok(value),
            Err(e) => {
                let caller_location = Location::caller();
                let mut app_err = AppError::new(e);
                app_err.add_enrichment(EnrichmentEntry::new(
                    msg_fn().to_string(),
                    caller_location.file(),
                    caller_location.line(),
                ));
                Err(app_err)
            }
        }
    }
}

impl<T> IntoAppError<T> for Option<T> {
    fn into_app_err(self, msg: impl Display) -> Result<T, AppError> {
        self.ok_or_else(|| AppError::new(msg.to_string()))
    }

    fn into_app_err_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.ok_or_else(|| AppError::new(msg_fn().to_string()))
    }
}

/// Specialized implementation for `Result<T, AppError>` to avoid double wrapping.
impl<T> IntoAppError<T> for Result<T, AppError> {
    #[track_caller]
    fn into_app_err(mut self, msg: impl Display) -> Self {
        if let Err(e) = &mut self {
            let caller_location = Location::caller();
            e.add_enrichment(EnrichmentEntry::new(
                msg.to_string(),
                caller_location.file(),
                caller_location.line(),
            ));
        }
        self
    }

    #[track_caller]
    fn into_app_err_with<F, D>(mut self, msg_fn: F) -> Self
    where
        F: FnOnce() -> D,
        D: Display,
    {
        if let Err(e) = &mut self {
            let caller_location = Location::caller();
            e.add_enrichment(EnrichmentEntry::new(
                msg_fn().to_string(),
                caller_location.file(),
                caller_location.line(),
            ));
        }
        self
    }
}
