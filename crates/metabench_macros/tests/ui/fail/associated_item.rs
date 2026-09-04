// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

struct Group;

#[benchmarks]
impl Group {
    const VALUE: usize = 1;
}

fn main() {}
