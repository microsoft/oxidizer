// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with aggregate telemetry enabled.

mod workloads;

rallocator::config!(AggregatesConfig { track_aggregates: true });
rallocator::rallocator!(AggregatesConfig);

fn main() {
    workloads::run("tracking_aggregates");
}
