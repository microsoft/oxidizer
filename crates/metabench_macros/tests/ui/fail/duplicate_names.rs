// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

struct Group;

#[benchmarks]
impl Group {
    #[metabench_macros::benchmark(name = "same")]
    fn first() {}

    #[metabench_macros::benchmark(name = "same")]
    fn second() {}
}

fn main() {}
