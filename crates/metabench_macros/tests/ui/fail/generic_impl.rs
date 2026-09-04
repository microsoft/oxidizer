// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

struct Group<T>(T);

#[benchmarks]
impl<T> Group<T> {
    fn work() {}
}

fn main() {}
