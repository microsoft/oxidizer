// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! General allocation workload benchmark using mimalloc.

mod workloads;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    workloads::run("mimalloc");
}
