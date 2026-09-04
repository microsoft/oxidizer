// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

struct Group;

#[benchmarks(name = "invalid/name")]
impl Group {
    fn work() {}
}

fn main() {}
