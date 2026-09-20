// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

zygote_rt::link!();

fn main() {
    let code = zygote_test_targets::run(std::env::args_os().collect(), None);
    if code != 0 {
        std::process::exit(code);
    }
}
