// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump workload benchmark using rallocator's global heap.

mod bump_workloads;
mod ordinary_workloads;

rallocator::rallocator!();

fn main() {
    bump_workloads::run("bump_rallocator_global", ordinary_workloads::WORKLOADS);
}
