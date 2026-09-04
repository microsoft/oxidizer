// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with event recording but without backtraces.

mod workloads;

rallocator::rallocator!();

fn main() {
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });
    workloads::run("rallocator_tracking_events");
}
