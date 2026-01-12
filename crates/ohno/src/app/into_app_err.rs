// Copyright (c) Microsoft Corporation.

use std::fmt::Display;
use std::panic::Location;

use crate::{Enrichable, EnrichmentEntry};

use super::AppError;

/// Transforms `Result` type into `AppError` with additional message
/// 
/// For converting an error into `AppError` without additional context, use `?` operator directly.
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
    fn into_app_err(self, msg: impl Display) -> Result<T, AppError> {
        self.map_err(|e| {
            let caller = Location::caller();
            let mut e = AppError::new(e);
            e.add_enrichment(EnrichmentEntry::new(msg.to_string(), caller.file(), caller.line()));
            e
        })
    }

    fn into_app_err_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.map_err(|e| {
            let caller = Location::caller();
            let mut e = AppError::new(e);
            e.add_enrichment(EnrichmentEntry::new(msg_fn().to_string(), caller.file(), caller.line()));
            e
        })
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
    fn into_app_err(self, msg: impl Display) -> Result<T, AppError> {
        self.map_err(|mut e| {
            let caller = Location::caller();
            e.add_enrichment(EnrichmentEntry::new(msg.to_string(), caller.file(), caller.line()));
            e
        })
    }

    fn into_app_err_with<F, D>(self, msg_fn: F) -> Result<T, AppError>
    where
        F: FnOnce() -> D,
        D: Display,
    {
        self.map_err(|mut e| {
            let caller = Location::caller();
            e.add_enrichment(EnrichmentEntry::new(msg_fn().to_string(), caller.file(), caller.line()));
            e
        })
    }
}
