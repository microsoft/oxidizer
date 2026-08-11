// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump workload benchmark using the system allocator.

mod bump_workloads;
mod ordinary_workloads;

fn main() {
    bump_workloads::run("bump_system", ordinary_workloads::WORKLOADS);
}
