// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[renamed_metabench::benchmark(CONST_GENERIC, "group", "const-generic")]
#[bench::array(args = ([1_u8, 2],), consts = [2])]
fn workload<const N: usize>(value: [u8; N]) -> usize {
    value.len()
}

fn main() {}
