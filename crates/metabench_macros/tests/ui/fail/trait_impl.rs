// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

trait Work {
    fn work();
}

struct Group;

#[benchmarks]
impl Work for Group {
    fn work() {}
}

fn main() {}
