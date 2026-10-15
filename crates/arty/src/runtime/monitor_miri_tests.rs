// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Replay public live-query ownership scenarios with private instrumentation-only fixtures.
//! Native and careful tests exercise the real endpoint; no endpoint is faked here.

macro_rules! monitored_runtime_builder {
    ($name:expr) => {
        crate::runtime::Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .monitor(crate::runtime::MonitorOptions::instrumentation_only_for_tests().name($name))
    };
}

#[path = "../../tests/support/live_diagnostics_cases.rs"]
mod cases;

#[path = "../../tests/support/seismograph_cases.rs"]
mod recording_cases;
