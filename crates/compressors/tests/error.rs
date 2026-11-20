// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Foreign error sources and recovery information.

use std::error::Error as StdError;
use std::fmt;

use compressors::Error;
use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

#[test]
fn is_source_reports_only_the_source_kind() {
    let error = Error::other("the underlying stream failed", std::io::Error::other("stream failed"));

    assert!(error.is_source(), "got {error}");
    assert!(!error.is_corrupt_data(), "got {error}");
    assert!(!error.is_invalid_configuration(), "got {error}");
}

#[test]
fn a_wrapped_cause_can_be_taken_back_by_value() {
    let error = Error::other("the source stopped", std::io::Error::other("the cause"));

    assert!(error.source().is_some(), "the cause must also be visible by reference");

    let taken = error.into_source().unwrap();
    let taken = taken.downcast::<std::io::Error>().unwrap();
    assert_eq!(taken.to_string(), "the cause");
}

#[test]
fn other_classifies_a_wrapped_io_error_by_its_kind() {
    let cases = [
        (std::io::ErrorKind::TimedOut, RecoveryKind::Retry),
        (std::io::ErrorKind::NetworkDown, RecoveryKind::Unavailable),
        (std::io::ErrorKind::NotFound, RecoveryKind::Never),
    ];

    for (kind, expected) in cases {
        let error = Error::other("the transport failed", std::io::Error::from(kind));

        assert!(error.is_source(), "got {error}");
        assert_eq!(error.to_string(), "the transport failed");
        assert_eq!(error.recovery().kind(), expected, "wrong recovery for {kind:?}");
    }
}

#[test]
fn other_finds_an_io_error_nested_inside_a_wrapper() {
    #[derive(Debug)]
    struct Wrapper(std::io::Error);

    impl fmt::Display for Wrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("wrapped")
        }
    }

    impl StdError for Wrapper {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&self.0)
        }
    }

    let error = Error::other(
        "the transport failed",
        Wrapper(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
    );

    assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    assert_eq!(
        error.source().unwrap().to_string(),
        "wrapped",
        "the wrapper itself stays the reported cause, not the io::Error the heuristic reached through it"
    );
}

#[test]
fn other_reports_unknown_when_nothing_in_the_chain_is_an_io_error() {
    let error = Error::other("something else failed", "a plain message");

    assert_eq!(error.recovery().kind(), RecoveryKind::Unknown);
    assert_eq!(error.source().unwrap().to_string(), "a plain message");
}

#[test]
fn other_with_recovery_attaches_what_it_was_given() {
    let supplied = RecoveryInfo::unavailable();
    let error = Error::other_with_recovery("the backend is degraded", "a plain message", supplied.clone());

    assert!(error.is_source(), "got {error}");
    assert_eq!(
        error.recovery(),
        supplied,
        "the supplied information must take precedence over the heuristic"
    );
}

#[test]
fn other_with_recovery_overrides_what_the_heuristic_would_have_detected() {
    let error = Error::other_with_recovery(
        "the deadline is gone",
        std::io::Error::from(std::io::ErrorKind::TimedOut),
        RecoveryInfo::never(),
    );

    assert_eq!(error.recovery().kind(), RecoveryKind::Never);
}

#[test]
fn a_wrapped_cause_carries_its_own_recovery() {
    let error = Error::other(
        "the underlying stream failed",
        std::io::Error::from(std::io::ErrorKind::ConnectionReset),
    );

    assert!(error.is_source(), "got {error}");
    assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
}
