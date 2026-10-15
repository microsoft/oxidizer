// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build-time compiler, target and profile metadata.

use std::env;
use std::process::Command;

fn main() {
    let target = env::var("TARGET").expect("Cargo always sets TARGET");
    let profile = env::var("PROFILE").expect("Cargo always sets PROFILE");
    let rustc = env::var_os("RUSTC").expect("Cargo always sets RUSTC");
    let output = Command::new(rustc)
        .arg("-Vv")
        .output()
        .expect("Cargo's selected compiler must be executable");
    assert!(output.status.success(), "Cargo's selected compiler failed to report its version");
    let version = String::from_utf8(output.stdout).expect("rustc -Vv emits UTF-8 version information");
    println!("cargo::rustc-env=METABENCH_RUSTC_VERSION={}", version.trim().replace('\n', " | "));
    println!("cargo::rerun-if-env-changed=RUSTC");
    println!("cargo::rustc-env=METABENCH_TARGET={target}");
    println!("cargo::rustc-env=METABENCH_PROFILE={profile}");
    if let Ok(flags) = env::var("CARGO_ENCODED_RUSTFLAGS") {
        println!("cargo::rustc-env=METABENCH_RUSTFLAGS={}", flags.replace('\u{1f}', " "));
    }
}
