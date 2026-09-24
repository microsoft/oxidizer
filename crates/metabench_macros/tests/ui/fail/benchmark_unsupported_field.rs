// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(
    identity = IDENTITY,
    group_name = "group",
    benchmark_name = "name",
    unknown = true
)]
fn workload() {}

fn main() {}
