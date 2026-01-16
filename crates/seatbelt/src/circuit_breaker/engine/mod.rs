// Copyright (c) Microsoft Corporation.

use std::fmt::Debug;
use std::time::Duration;

use crate::circuit_breaker::{ExecutionResult, HealthInfo, HealthMetricsBuilder};

pub(super) mod probing;

#[derive(Debug, Copy, Clone)]
pub(crate) enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// Result of attempting to enter the circuit.
#[derive(Debug, Clone)]
pub(crate) enum EnterCircuitResult {
    /// The operation is allowed to proceed.
    ///
    /// The `probe` indicates that this is a test operation used to evaluate whether
    /// the circuit can be closed again.
    Accepted { mode: ExecutionMode },

    /// Operation is rejected due to open circuit.
    Rejected,
}

#[derive(Debug, Clone)]
pub(crate) enum ExitCircuitResult {
    /// The state remains unchanged.
    Unchanged,

    /// Circuit transitioned to Open state.
    Opened(HealthInfo),

    /// Circuit re-transitioned to Open state due to a failure in Half-Open state.
    Reopened,

    /// Circuit transitioned back to Closed state.
    Closed(Stats),
}

/// Configuration options for the circuit breaker engine.
#[derive(Debug, Clone)]
pub(crate) struct EngineOptions {
    pub break_duration: Duration,
    pub health_metrics_builder: HealthMetricsBuilder,
    pub probes: probing::ProbesOptions,
}

/// Determines the mode of execution for an operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ExecutionMode {
    /// Regular operation.
    Normal,

    /// A probe operation to test the health of the underlying service.
    Probe,
}

// Type alias for the default engine with telemetry.
pub type Engine = EngineTelemetry<EngineCore>;

/// Trait defining the behavior of a circuit breaker engine.
pub(crate) trait CircuitEngine: Debug + Send + Sync + 'static {
    fn enter(&self) -> EnterCircuitResult;

    fn exit(&self, result: ExecutionResult, mode: ExecutionMode) -> ExitCircuitResult;
}

mod engine_core;
pub(crate) use engine_core::*;

#[cfg(test)]
mod engine_fake;
#[cfg(test)]
pub(crate) use engine_fake::*;

mod engine_telemetry;
pub(crate) use engine_telemetry::*;

mod engines;
pub(crate) use engines::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_state_as_str() {
        assert_eq!(CircuitState::Closed.as_str(), "closed");
        assert_eq!(CircuitState::Open.as_str(), "open");
        assert_eq!(CircuitState::HalfOpen.as_str(), "half_open");
    }
}
