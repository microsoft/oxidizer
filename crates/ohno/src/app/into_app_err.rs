// Copyright (c) Microsoft Corporation.

use std::fmt::Display;

use crate::Enrichable;

use super::AppError;

/// Transforms `Result` type into `AppError` with additional message
pub trait IntoAppError<T> {
    /// Adds context message to the error if the result is an error.
    fn ohno(self, msg: impl Display) -> Result<T, AppError>;

    /// Adds context message to the error if the result is an error.
    ///
    /// The function `msg_fn` is only called if the result is an error.
    fn ohno_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display;
}

impl<T, E> IntoAppError<T> for Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn ohno(self, msg: impl Display) -> Result<T, AppError> {
        self.map_err(|e| {
            let caller = std::panic::Location::caller();
            let mut e = AppError::new(e);
            e.add_enrichment(crate::EnrichmentEntry::new(msg.to_string(), caller.file(), caller.line()));
            e
        })
    }

    fn ohno_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.map_err(|e| {
            let caller = std::panic::Location::caller();
            let mut e = AppError::new(e);
            e.add_enrichment(crate::EnrichmentEntry::new(msg_fn().to_string(), caller.file(), caller.line()));
            e
        })
    }
}

impl<T> IntoAppError<T> for Option<T> {
    fn ohno(self, msg: impl Display) -> Result<T, AppError> {
        self.ok_or_else(|| AppError::new(msg.to_string()))
    }

    fn ohno_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.ok_or_else(|| AppError::new(msg_fn().to_string()))
    }
}

impl<T> IntoAppError<T> for Result<T, AppError> {
    fn ohno(self, msg: impl Display) -> Result<T, AppError> {
        self.map_err(|mut e| {
            let caller = std::panic::Location::caller();
            e.add_enrichment(crate::EnrichmentEntry::new(msg.to_string(), caller.file(), caller.line()));
            e
        })
    }

    fn ohno_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.map_err(|mut e| {
            let caller = std::panic::Location::caller();
            e.add_enrichment(crate::EnrichmentEntry::new(msg_fn().to_string(), caller.file(), caller.line()));
            e
        })
    }
}
