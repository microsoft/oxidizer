// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with all standard telemetry enabled.

mod workloads;

rallocator::config!(AllTrackingConfig {
    track_aggregates: true,
    track_callers: true,
});
rallocator::rallocator!(AllTrackingConfig);

fn main() {
    workloads::run("tracking_all");
}
