// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multithreaded allocation benchmarks using mimalloc.

//! Multithreaded allocation benchmark using mimalloc.

mod rallocator_threaded_workloads;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    rallocator_threaded_workloads::run("rallocator_threaded_mimalloc");
}
