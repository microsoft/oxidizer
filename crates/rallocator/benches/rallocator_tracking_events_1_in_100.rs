// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark sampling one in 100 objects without backtraces.

mod workloads;

rallocator::rallocator!();

fn main() {
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled: true,
            event_sampling: seismograph::recorder::EventSampling::one_in(100).expect("100 is within the supported nonzero sampling range"),
            ..Default::default()
        },
        ..Default::default()
    });
    workloads::run("rallocator_tracking_events_1_in_100");
}
