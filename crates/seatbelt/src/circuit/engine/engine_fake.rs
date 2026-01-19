// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::circuit::{CircuitEngine, EnterCircuitResult, ExecutionMode, ExecutionResult, ExitCircuitResult};

/// Fake engine to be used in tests.
#[derive(Debug)]
pub(crate) struct EngineFake {
    enter_result: EnterCircuitResult,
    exit_result: ExitCircuitResult,
}

impl EngineFake {
    pub fn new(enter_result: EnterCircuitResult, exit_result: ExitCircuitResult) -> Self {
        Self { enter_result, exit_result }
    }
}

impl CircuitEngine for EngineFake {
    fn enter(&self) -> EnterCircuitResult {
        self.enter_result.clone()
    }

    fn exit(&self, _result: ExecutionResult, _mode: ExecutionMode) -> ExitCircuitResult {
        self.exit_result.clone()
    }
}
