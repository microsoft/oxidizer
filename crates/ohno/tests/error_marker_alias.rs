// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An explicit `#[error]` marker designates the field holding the `OhnoCore`, whatever the field's
//! type is spelled as.
//!
//! Auto-detection reads the final path segment, so it never finds a core reached through a type
//! alias or an import rename. The marker is the only way to designate one, which is why the marked
//! field's type is left to `rustc` rather than checked by spelling.

use std::error::Error;

use ohno::{ErrorExt, OhnoCore};

/// A type alias for the core.
type Core = OhnoCore;

/// The core under an import rename.
use ohno::OhnoCore as RenamedCore;

/// An error whose core is reached through a type alias.
#[derive(ohno::Error)]
#[display("alias failed for {path}")]
pub struct AliasedCoreError {
    /// The path the operation failed for.
    pub path: String,
    #[error]
    inner: Core,
}

/// An error whose core is reached through an import rename.
#[derive(ohno::Error)]
#[display("rename failed for {path}")]
pub struct RenamedCoreError {
    /// The path the operation failed for.
    pub path: String,
    #[error]
    inner: RenamedCore,
}

/// An error whose core is reached through an alias in tuple position.
#[derive(ohno::Error)]
#[display("tuple alias failed for {0}")]
pub struct TupleAliasedCoreError(pub String, #[error] Core);

#[test]
fn aliased_core_is_the_error_representation() {
    let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "root cause");
    let error = AliasedCoreError::caused_by("/etc/config.toml".to_string(), cause);

    let rendered = error.to_string();
    assert!(rendered.starts_with("alias failed for /etc/config.toml"), "got: {rendered}");
    assert!(rendered.contains("caused by: root cause"), "got: {rendered}");

    assert_eq!(error.source().expect("source").to_string(), "root cause");
}

#[test]
fn renamed_core_is_the_error_representation() {
    let cause = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let error = RenamedCoreError::caused_by("/tmp/x".to_string(), cause);

    assert!(error.to_string().starts_with("rename failed for /tmp/x"), "got: {error}");
    assert_eq!(error.source().expect("source").to_string(), "missing");
}

#[test]
fn aliased_core_in_tuple_position_is_the_error_representation() {
    let cause = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
    let error = TupleAliasedCoreError::caused_by("/var/run".to_string(), cause);

    assert!(error.to_string().starts_with("tuple alias failed for /var/run"), "got: {error}");
    assert_eq!(error.source().expect("source").to_string(), "timed out");
}

#[test]
fn aliased_core_carries_no_source_when_nothing_is_wrapped() {
    let error = AliasedCoreError::new("/tmp/y".to_string());

    assert_eq!(error.message(), "alias failed for /tmp/y");
    assert!(error.source().is_none());
}
