// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(VARIADIC, "group", "variadic")]
unsafe extern "C" fn workload(first: u64, ...) {
    let _ = first;
}

fn main() {}
