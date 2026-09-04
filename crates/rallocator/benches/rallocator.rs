// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! General allocation workload benchmark using rallocator.

mod workloads;

rallocator::rallocator!();

fn main() {
    workloads::run("rallocator");
}
