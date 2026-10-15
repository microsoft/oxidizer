// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime recording contracts over runtime-owned native diagnostic endpoints.
//! Miri replays the same recording cases through a private instrumentation fixture.

#![cfg(all(feature = "rt", feature = "seismograph", not(miri)))]

macro_rules! monitored_runtime_builder {
    ($name:expr) => {
        arty::runtime::Runtime::builder().monitor(arty::runtime::MonitorOptions::default().name($name))
    };
}

#[path = "support/seismograph_cases.rs"]
mod cases;
