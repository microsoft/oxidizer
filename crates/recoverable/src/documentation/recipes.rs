// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pass recovery information between error types.
//!
//! These examples show how to implement [`Recovery`].
//! They use [`ohno`](https://docs.rs/ohno) to define errors.
//!
//! # Pass information from an inner error
//!
//! An outer error can wrap an inner error that implements [`Recovery`]. Use the
//! `#[from]` attribute to copy its recovery information. This preserves the
//! classification from the layer that found the root cause.
//!
//! ```rust
//! use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
//!
//! #[ohno::error]
//! struct DatabaseError {
//!     recovery: RecoveryInfo,
//! }
//!
//! impl Recovery for DatabaseError {
//!     fn recovery(&self) -> RecoveryInfo {
//!         self.recovery.clone()
//!     }
//! }
//!
//! /// The `#[from]` attribute copies recovery information from `DatabaseError`.
//! /// The conversion calls `error.recovery()`.
//! #[ohno::error]
//! #[from(DatabaseError(recovery: error.recovery()))]
//! struct ServiceError {
//!     recovery: RecoveryInfo,
//! }
//!
//! impl Recovery for ServiceError {
//!     fn recovery(&self) -> RecoveryInfo {
//!         self.recovery.clone()
//!     }
//! }
//!
//! fn database_operation() -> Result<(), DatabaseError> {
//!     Err(DatabaseError::caused_by(
//!         RecoveryInfo::retry(),
//!         "connection timed out",
//!     ))
//! }
//!
//! fn service_operation() -> Result<(), ServiceError> {
//!     // The ? operator converts DatabaseError into ServiceError.
//!     // The #[from] attribute copies the recovery information.
//!     database_operation()?;
//!     Ok(())
//! }
//!
//! let err = service_operation().unwrap_err();
//! assert_eq!(err.recovery().kind(), RecoveryKind::Retry);
//! ```
//!
//! Override the inner classification only when the outer context changes it.
//! For example, an outer layer can exhaust its retry limit and classify a
//! transient inner error as non-recoverable.
//!
//! # Non-recoverable errors
//!
//! If a permanent error has no inner cause, use [`RecoveryInfo::never()`].
//!
//! If every variant is permanently non-recoverable, the error does not need
//! [`Recovery`]. If at least one variant can recover, implement the trait.
//! Also implement it if a future version can change the classification.
//!
//! ```rust
//! use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
//!
//! #[ohno::error]
//! struct ConfigError {
//!     recovery: RecoveryInfo,
//! }
//!
//! impl Recovery for ConfigError {
//!     fn recovery(&self) -> RecoveryInfo {
//!         self.recovery.clone()
//!     }
//! }
//!
//! let err = ConfigError::caused_by(
//!     RecoveryInfo::never(),
//!     "missing required field: database_url",
//! );
//! assert_eq!(err.recovery().kind(), RecoveryKind::Never);
//! ```
//!
//! # Heuristic recovery
//!
//! An inner error does not always implement [`Recovery`]. You can classify it
//! from a known property. For example, find a [`std::io::Error`] at the root of
//! the error chain. Then convert its [`ErrorKind`](std::io::ErrorKind) with
//! [`RecoveryInfo::from`].
//!
//! Use the `#[from]` attribute to classify the error during conversion.
//!
//! ```rust
//! use std::io;
//!
//! use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
//!
//! /// `std::io::Error` does not implement `Recovery`.
//! /// This conversion classifies its `ErrorKind`.
//! #[ohno::error]
//! #[from(io::Error(recovery: error.kind().into()))]
//! struct StorageError {
//!     recovery: RecoveryInfo,
//! }
//!
//! impl Recovery for StorageError {
//!     fn recovery(&self) -> RecoveryInfo {
//!         self.recovery.clone()
//!     }
//! }
//!
//! // A connection reset is transient.
//! let err = StorageError::from(io::Error::from(io::ErrorKind::ConnectionReset));
//! assert_eq!(err.recovery().kind(), RecoveryKind::Retry);
//!
//! // A "not found" error is permanent.
//! let err = StorageError::from(io::Error::from(io::ErrorKind::NotFound));
//! assert_eq!(err.recovery().kind(), RecoveryKind::Never);
//! ```
//!
//! An I/O error can occur deeper in the cause chain.
//! Use [`Error::source`](std::error::Error::source) to inspect each cause.
//!
//! ```rust
//! use std::error::Error;
//! use std::io;
//!
//! use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
//!
//! #[ohno::error]
//! struct ProtocolError {
//!     recovery: RecoveryInfo,
//! }
//!
//! impl ProtocolError {
//!     fn from_source(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
//!         let source: Box<dyn Error + Send + Sync> = source.into();
//!         let recovery = io_recovery_from_chain(source.as_ref());
//!         Self::caused_by(recovery, source)
//!     }
//! }
//!
//! impl Recovery for ProtocolError {
//!     fn recovery(&self) -> RecoveryInfo {
//!         self.recovery.clone()
//!     }
//! }
//!
//! /// Finds a `std::io::Error` in the error chain.
//! /// It classifies the error from its `ErrorKind`.
//! fn io_recovery_from_chain(err: &(dyn Error + 'static)) -> RecoveryInfo {
//!     std::iter::successors(Some(err), |e| (*e).source())
//!         .find_map(|e| e.downcast_ref::<io::Error>())
//!         .map(|io_err| RecoveryInfo::from(io_err.kind()))
//!         // If there is no I/O error, classify the condition as non-recoverable.
//!         .unwrap_or_else(RecoveryInfo::never)
//! }
//! ```
//!
//! The `From<ErrorKind> for RecoveryInfo` conversion defines defaults for common
//! I/O errors. Its API documentation lists every classification. If a default
//! does not match your use case, implement your own mapping.

#[expect(unused_imports, reason = "keeps documentation examples concise")]
use crate::*;
