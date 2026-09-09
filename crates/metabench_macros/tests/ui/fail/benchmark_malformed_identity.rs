// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(
    identity = module::IDENTITY,
    group_name = "group",
    benchmark_name = "name"
)]
fn workload() {}

fn main() {}
