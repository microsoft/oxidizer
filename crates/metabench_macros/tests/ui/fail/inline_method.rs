// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

struct Group;

#[benchmarks]
impl Group {
    #[inline]
    #[metabench_macros::benchmark]
    fn work() {}
}

fn main() {}
