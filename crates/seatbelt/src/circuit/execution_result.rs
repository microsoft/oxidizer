// Copyright (c) Microsoft Corporation.

use crate::RecoveryInfo;

/// An evaluated execution result.
///
/// From the perspective of a circuit breaker, an execution can either
/// succeed or fail. This enum captures that binary outcome.
#[derive(Debug, PartialEq, Copy, Clone)]
pub(crate) enum ExecutionResult {
    Success,
    Failure,
}

impl ExecutionResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

impl ExecutionResult {
    pub fn from_recovery(recovery: &RecoveryInfo) -> Self {
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
    }
}
