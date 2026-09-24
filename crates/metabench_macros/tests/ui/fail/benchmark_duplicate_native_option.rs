// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(
    identity = IDENTITY,
    group_name = "group",
    benchmark_name = "name",
    gungraun_config = first(),
    gungraun_config = second()
)]
fn workload() {}

fn main() {}
