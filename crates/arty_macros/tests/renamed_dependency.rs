// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumer compilation contract for a Cargo-renamed Arty dependency.

use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
#[cfg_attr(miri, ignore)]
fn entrypoint_macros_resolve_a_renamed_arty_dependency() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_root.parent().and_then(Path::parent).unwrap();
    let arty = workspace_root.join("crates").join("arty");
    let plurality = workspace_root.join("crates").join("plurality");
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("src");
    fs::create_dir(&source).unwrap();
    fs::write(
        directory.path().join("Cargo.toml"),
        format!(
            r#"[workspace]

[package]
name = "arty-renamed-dependency-fixture"
version = "0.0.0"
edition = "2024"

[dependencies]
renamed_arty = {{ package = "arty", path = "{}" }}

[patch.crates-io]
plurality = {{ path = "{}" }}
"#,
            arty.to_string_lossy().replace('\\', "\\\\"),
            plurality.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    fs::write(
        source.join("main.rs"),
        r"use renamed_arty::task::Builtins;

#[renamed_arty::main]
async fn main(cx: Builtins) {
    cx.scheduler().spawn(async |_| ()).await.unwrap();
}
",
    )
    .unwrap();

    let output = Command::new(env!("CARGO"))
        .args(["check", "--offline", "--quiet", "--manifest-path"])
        .arg(directory.path().join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", directory.path().join("target"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "renamed dependency fixture failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
