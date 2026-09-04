// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with active caller tracking.

mod workloads;

rallocator::rallocator!();

fn main() {
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            ..Default::default()
        },
        ..Default::default()
    });
    workloads::run("tracking_all_callers");
}
