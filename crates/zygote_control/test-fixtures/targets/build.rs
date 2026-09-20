// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(coverage_nightly)");
    zygote_rt::build::configure();
}
