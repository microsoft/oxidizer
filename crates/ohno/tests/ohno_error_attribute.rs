// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` always injects the `OhnoCore` field and always generates the error
//! representation from it.
//!
//! A field of type `OhnoCore` declared by the user is therefore an ordinary field: the injected one
//! is the marked one, so it is what the implementations read, and the declared one is left to the
//! user to carry and reference.

use std::error::Error;

use ohno::OhnoCore;

/// An error declaring a core field of its own, alongside the one the attribute injects.
#[ohno::error]
#[display("failed for {path}, carrying {carried}")]
pub struct DeclaredCoreError {
    /// The path the operation failed for.
    pub path: String,
    /// A core the user carries themselves, which is not the error representation.
    pub carried: OhnoCore,
}

#[test]
fn declared_core_field_is_an_ordinary_field() {
    let carried = OhnoCore::without_backtrace(std::io::Error::new(std::io::ErrorKind::NotFound, "carried.txt"));
    let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "root cause");
    let error = DeclaredCoreError::caused_by("/etc/config.toml".to_string(), carried, cause);

    // The display template reaches the declared field like any other. The injected core appends a
    // backtrace when one is enabled, so the rendering is matched by part rather than whole.
    let rendered = error.to_string();
    assert!(
        rendered.starts_with("failed for /etc/config.toml, carrying carried.txt"),
        "got: {rendered}"
    );
    assert!(rendered.contains("caused by: root cause"), "got: {rendered}");

    // The source is the one wrapped by the injected field, not the one the declared field holds
    assert_eq!(error.source().expect("source").to_string(), "root cause");
    assert_eq!(error.carried.source().expect("carried source").to_string(), "carried.txt");
}

#[test]
fn declared_core_field_does_not_take_over_the_error_representation() {
    let carried = OhnoCore::without_backtrace(std::io::Error::new(std::io::ErrorKind::NotFound, "carried.txt"));
    let error = DeclaredCoreError::new("/tmp/x".to_string(), carried);

    // With nothing wrapped by the injected field, the error has no source even though the declared
    // field holds one
    assert!(error.source().is_none());
    assert!(error.carried.source().is_some());
}

/// The remedy `#[ohno::error]` names when it rejects a field.
///
/// Both `ALREADY_MARKED` and `RESERVED_MARKER` tell the user to place the core themselves with
/// `#[derive(ohno::Error)]` and mark it with `#[error]`. This is that struct, so the advice is
/// pinned by something that compiles rather than only by the text of the message.
#[derive(ohno::Error)]
#[display("failed for {path}")]
pub struct PlacedCoreError {
    /// The path the operation failed for.
    pub path: String,
    #[error]
    inner: OhnoCore,
}

#[test]
fn suggested_remedy_places_the_core_by_hand() {
    let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "root cause");
    let error = PlacedCoreError::caused_by("/etc/config.toml".to_string(), cause);

    assert!(error.to_string().starts_with("failed for /etc/config.toml"), "got: {error}");
    assert_eq!(error.source().expect("source").to_string(), "root cause");
}
