// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

struct Group;

#[benchmarks]
impl Group {
    #[metabench_macros::benchmark(name = "invalid/name")]
    fn work() {}
}

fn main() {}
