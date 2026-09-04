// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use metabench_macros::benchmarks;

#[benchmarks]
impl [u8; 4] {
    fn work() {}
}

fn main() {}
