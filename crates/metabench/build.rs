// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build-time target and profile metadata.

fn main() {
    let target = std::env::var("TARGET").expect("Cargo always sets TARGET");
    let profile = std::env::var("PROFILE").expect("Cargo always sets PROFILE");
    println!("cargo::rustc-env=METABENCH_TARGET={target}");
    println!("cargo::rustc-env=METABENCH_PROFILE={profile}");
    if let Ok(flags) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        println!("cargo::rustc-env=METABENCH_RUSTFLAGS={}", flags.replace('\u{1f}', " "));
    }
}
