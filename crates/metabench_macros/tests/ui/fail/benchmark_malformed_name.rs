// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(
    identity = IDENTITY,
    group_name = "group",
    benchmark_name = 42
)]
fn workload() {}

fn main() {}
