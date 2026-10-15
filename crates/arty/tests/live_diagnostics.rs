// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public monitoring integration over native endpoints.
//! Miri replays the same ownership cases from the library with a private
//! instrumentation-only fixture, while native/careful tests retain real TCP coverage.

#![cfg(all(feature = "rt", feature = "seismograph", not(miri)))]

macro_rules! monitored_runtime_builder {
    ($name:expr) => {
        arty::runtime::Runtime::builder()
            .workers(arty::runtime::WorkersPolicy::exactly(1))
            .monitor(arty::runtime::MonitorOptions::default().name($name))
    };
}

#[path = "support/live_diagnostics_cases.rs"]
mod cases;
