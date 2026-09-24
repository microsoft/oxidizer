// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with lifetime aggregate telemetry.

mod workloads;

rallocator::rallocator!();

fn main() {
    workloads::run("tracking_aggregates");
}
