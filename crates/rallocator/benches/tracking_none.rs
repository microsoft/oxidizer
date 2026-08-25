// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation benchmark with telemetry disabled.

mod workloads;

rallocator::rallocator!();

fn main() {
    workloads::run("tracking_none");
}
