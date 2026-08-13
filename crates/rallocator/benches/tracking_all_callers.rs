// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with active caller tracking.

mod workloads;

rallocator::config!(AllTrackingWithCallersConfig {
    track_aggregates: true,
    track_callers: true,
});
rallocator::rallocator!(AllTrackingWithCallersConfig);

fn main() {
    rallocator::telemetry::track_callers(true);
    workloads::run("tracking_all_callers");
}
