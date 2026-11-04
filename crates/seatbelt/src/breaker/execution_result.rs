// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::RecoveryInfo;

/// An evaluated execution result.
///
/// A circuit breaker records success, failure, or abandonment.
/// An abandoned execution started but did not finish.
#[derive(Debug, PartialEq, Copy, Clone)]
pub(crate) enum ExecutionResult {
    Success,
    Failure,

    /// The execution started, but its future was dropped or canceled.
    Abandoned,
}

#[cfg(any(feature = "logs", feature = "metrics", test))]
impl ExecutionResult {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Abandoned => "abandoned",
        }
    }
}

impl ExecutionResult {
    pub(crate) fn from_recovery(recovery: &RecoveryInfo) -> Self {
        match recovery.kind() {
            crate::RecoveryKind::Retry | crate::RecoveryKind::Unavailable => Self::Failure,
            _ => Self::Success,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_from_recovery() {
        assert_eq!(ExecutionResult::from_recovery(&RecoveryInfo::retry()), ExecutionResult::Failure);
        assert_eq!(
            ExecutionResult::from_recovery(&RecoveryInfo::unavailable()),
            ExecutionResult::Failure
        );
        assert_eq!(ExecutionResult::from_recovery(&RecoveryInfo::never()), ExecutionResult::Success);
        assert_eq!(ExecutionResult::from_recovery(&RecoveryInfo::unknown()), ExecutionResult::Success);
    }

    #[test]
    fn test_execution_result_as_str() {
        assert_eq!(ExecutionResult::Success.as_str(), "success");
        assert_eq!(ExecutionResult::Failure.as_str(), "failure");
        assert_eq!(ExecutionResult::Abandoned.as_str(), "abandoned");
    }
}
