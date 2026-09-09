// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(UNSAFE, "group", "unsafe")]
unsafe fn workload(value: u64) -> u64 {
    value
}

fn main() {}
