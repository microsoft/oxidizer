// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multithreaded allocation benchmarks using rallocator.

//! Multithreaded allocation benchmark using rallocator.

mod rallocator_threaded_workloads;

rallocator::rallocator!();

fn main() {
    rallocator::initialize();
    rallocator_threaded_workloads::run("rallocator_threaded");
}
