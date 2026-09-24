// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(
    identity = IDENTITY,
    group_name = "first",
    group_name = "second",
    benchmark_name = "name"
)]
fn workload() {}

fn main() {}
