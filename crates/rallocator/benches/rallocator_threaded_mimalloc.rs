// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Multithreaded allocation benchmarks using mimalloc.

//! Multithreaded allocation benchmark using mimalloc.

#[path = "rallocator_threaded_workloads/native_provenance.rs"]
mod native_provenance;
mod rallocator_threaded_workloads;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    let provenance = native_provenance::NativeProvenance::begin();
    rallocator_threaded_workloads::run("rallocator_threaded_mimalloc");
    if let Some(provenance) = provenance {
        provenance.finish();
    }
}
