// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump workload benchmark using mimalloc.

mod bump_workloads;
mod ordinary_workloads;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    bump_workloads::run("bump_mimalloc", ordinary_workloads::WORKLOADS);
}
