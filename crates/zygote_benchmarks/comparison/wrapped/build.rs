// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

fn main() {
    zygote_rt::build::configure();
    println!("cargo:rustc-cfg=zygote_bench_wrapped");
    println!("cargo:rustc-check-cfg=cfg(zygote_bench_wrapped)");
}
