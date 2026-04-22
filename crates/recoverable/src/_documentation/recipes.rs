// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recipes and patterns for flowing recovery information.
//!
//! A cookbook of practical examples for implementing [`Recovery`] effectively.
//! All examples use [`ohno`](https://docs.rs/ohno) for error definitions.
//!
//! # Flow from inner errors
//!
//! When your error wraps an inner error that already implements [`Recovery`],
//! flow its recovery information through the `#[from]` attribute. This
//! preserves the classification made by the layer closest to the root cause.
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
//! /// The `#[from]` attribute flows recovery info from `DatabaseError`
//! /// automatically — `error.recovery()` is called during conversion.
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
//!     Err(DatabaseError::caused_by(RecoveryInfo::retry(), "connection timed out"))
//! }
//!
//! fn service_operation() -> Result<(), ServiceError> {
//!     // The ? operator converts DatabaseError into ServiceError,
//!     // flowing recovery info automatically via #[from].
//!     database_operation()?;
//!     Ok(())
//! }
//!
//! let err = service_operation().unwrap_err();
//! assert_eq!(err.recovery().kind(), RecoveryKind::Retry);
//! ```
//!
//! Only override the inner classification when the outer context changes the
//! recoverability. For example, a transient inner error might become
//! non-recoverable if a retry budget is exhausted at the outer layer.
//!
//! # Non-recoverable errors
//!
//! When an error has no inner cause and the condition is permanent, use
//! [`RecoveryInfo::never()`] directly.
//!
//! If **all** states of an error are permanently non-recoverable and this is
//! unlikely to change, you do not need to implement [`Recovery`] at all.
//! Only implement the trait when at least some states may be recoverable or
//! when the recoverability classification may evolve in the future.
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
//! let err = ConfigError::caused_by(RecoveryInfo::never(), "missing required field: database_url");
//! assert_eq!(err.recovery().kind(), RecoveryKind::Never);
//! ```
//!
//! # Heuristic recovery
//!
//! Sometimes an inner error does not implement [`Recovery`] but the operation
//! is potentially recoverable. In these cases, use heuristics to derive
//! recovery information. A common pattern is detecting [`std::io::Error`] as
//! the root cause and converting its [`ErrorKind`](std::io::ErrorKind) into
//! [`RecoveryInfo`] via the built-in [`RecoveryInfo::from`]
//! conversion.
//!
//! The `#[from]` attribute lets you apply the heuristic at conversion time:
//!
//! ```rust
//! use std::io;
//!
//! use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
//!
//! /// `std::io::Error` does not implement `Recovery`, so we derive
//! /// recoverability from its `ErrorKind` using the built-in conversion.
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
//! // A connection-reset IO error is classified as transient.
//! let err = StorageError::from(io::Error::from(io::ErrorKind::ConnectionReset));
//! assert_eq!(err.recovery().kind(), RecoveryKind::Retry);
//!
//! // A "not found" IO error is classified as permanent.
//! let err = StorageError::from(io::Error::from(io::ErrorKind::NotFound));
//! assert_eq!(err.recovery().kind(), RecoveryKind::Never);
//! ```
//!
//! When the IO error is buried deeper in the cause chain and you need to
//! walk it manually, use [`Error::source`](std::error::Error::source):
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
//! /// Walk the error chain looking for a `std::io::Error` and derive
//! /// recovery info from its `ErrorKind`.
//! fn io_recovery_from_chain(err: &(dyn Error + 'static)) -> RecoveryInfo {
//!     std::iter::successors(Some(err), |e| (*e).source())
//!         .find_map(|e| e.downcast_ref::<io::Error>())
//!         .map(|io_err| RecoveryInfo::from(io_err.kind()))
//!         // No IO error found — assume non-recoverable.
//!         .unwrap_or_else(RecoveryInfo::never)
//! }
//! ```
//!
//! The [`From<ErrorKind>`](RecoveryInfo#impl-From<ErrorKind>-for-RecoveryInfo)
//! conversion provides opinionated defaults for common IO error kinds.
//! See its documentation for the full classification table.
//! If the defaults don't match your use case, implement your own mapping.

#[expect(unused_imports, reason = "simplifies the docs")]
use crate::*;
