// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recoverability detection for HTTP errors.

use std::error::Error as StdError;

use http_extensions::HttpError;
use seatbelt::{Recovery, RecoveryInfo, RecoveryKind};

/// Determines if an error is recoverable by analyzing the error chain.
pub(crate) fn detect_recoverability(error: &(dyn StdError + 'static)) -> RecoveryInfo {
    let recoverability = for_http_error(error);

    if recoverability.kind() == RecoveryKind::Unknown {
        return for_other_errors(error);
    }

    recoverability
}

fn for_other_errors(error: &(dyn StdError + 'static)) -> RecoveryInfo {
    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        return io_error.kind().into();
    }

    if let Some(hyper_error) = error.downcast_ref::<hyper::Error>() {
        return from_hyper_error(hyper_error);
    }

    error.source().map_or(RecoveryInfo::unknown(), for_other_errors)
}

fn from_hyper_error(error: &hyper::Error) -> RecoveryInfo {
    match error {
        _ if error.is_canceled() => RecoveryInfo::retry(),
        _ if error.is_timeout() => RecoveryInfo::retry(),
        _ if error.is_closed() => RecoveryInfo::retry(),
        _ if error.is_body_write_aborted() => RecoveryInfo::retry(),
        _ => RecoveryInfo::never(),
    }
}

fn for_http_error(error: &(dyn StdError + 'static)) -> RecoveryInfo {
    if let Some(http_error) = error.downcast_ref::<HttpError>() {
        let recoverability = http_error.recovery();

        if recoverability.kind() != RecoveryKind::Unknown {
            return recoverability;
        }
    }

    error.source().map_or(RecoveryInfo::unknown(), for_http_error)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::ErrorKind;

    use super::*;
    use crate::testing::{TestError, create_hyper_error};

    #[test]
    fn unknown_error_returns_unknown() {
        let err = TestError::new("dummy");
        assert_eq!(detect_recoverability(&err), RecoveryInfo::unknown());
    }

    #[test]
    fn http_error_recoverability_takes_priority() {
        let validation = HttpError::validation("test validation error");
        assert_eq!(detect_recoverability(&validation), RecoveryInfo::never());

        let io = HttpError::from(std::io::Error::new(ErrorKind::TimedOut, "timeout"));
        assert_eq!(detect_recoverability(&io), RecoveryInfo::retry());
    }

    #[test]
    fn error_chain_traversal_finds_recoverable_io_error() {
        let io = std::io::Error::new(ErrorKind::TimedOut, "timeout");
        let inner = TestError::new("inner").with_inner(io);
        let outer = TestError::new("outer").with_inner(inner);
        assert_eq!(detect_recoverability(&outer), RecoveryInfo::retry());
    }

    #[test]
    fn http_error_priority_over_io_error_in_chain() {
        let http_err = HttpError::validation("validation error");
        let wrapped_http = TestError::new("wrapped").with_inner(http_err);
        let wrapped_io = std::io::Error::other(wrapped_http);
        let top = TestError::new("top").with_inner(wrapped_io);
        assert_eq!(detect_recoverability(&top), RecoveryInfo::never());
    }

    #[test]
    fn hyper_error_path_returns_never_for_unrelated_failure() {
        let hyper_err = create_hyper_error();
        assert_eq!(from_hyper_error(&hyper_err), RecoveryInfo::never());
    }

    #[test]
    fn io_error_kind_drives_recoverability() {
        let timed_out = std::io::Error::new(ErrorKind::TimedOut, "timeout");
        assert_eq!(detect_recoverability(&timed_out), RecoveryInfo::retry());
    }
}
