// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumer-facing compilation contracts for the procedural macros.

#![expect(clippy::unwrap_used, reason = "a launch failure should fail the compile-contract test")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use tempfile::TempDir;

const PASS_CASES: &[&str] = &["benchmark_contracts", "generated_name_collisions", "main_contracts"];
const FAIL_CASES: &[(&str, &str)] = &[
    (
        "async_function",
        "async benchmark functions are unsupported because Gungraun does not await returned futures",
    ),
    ("benchmark_duplicate_field", "`identity` may only be specified once"),
    ("benchmark_duplicate_group_name", "`group_name` may only be specified once"),
    ("benchmark_duplicate_native_option", "`gungraun_config` may only be specified once"),
    ("benchmark_malformed_identity", "`identity` must be a single Rust identifier"),
    ("benchmark_malformed_name", "`benchmark_name` must be a string literal"),
    ("benchmark_missing_field", "missing `benchmark_name`"),
    ("benchmark_mixed_forms", "positional and named benchmark arguments cannot be mixed"),
    ("benchmark_unsupported_field", "unsupported benchmark option"),
    (
        "const_generic_function",
        "const-generic benchmark functions are unsupported because Gungraun cannot forward the adapter const parameter",
    ),
    ("receiver", "benchmark functions cannot have a receiver"),
    (
        "unsafe_function",
        "unsafe benchmark functions are unsupported because the generated adapter cannot uphold their safety contract",
    ),
    (
        "variadic_function",
        "variadic benchmark functions are unsupported because their arguments cannot be forwarded",
    ),
    ("target_duplicate_benchmark", "a benchmark identity may only belong to one group"),
    ("target_duplicate_group", "a group may only be declared once"),
    ("target_duplicate_option", "group option may only be specified once"),
    ("target_malformed_field", "`gungraun_compare_by_id` must be a boolean literal"),
    ("target_missing_benchmarks", "missing `benchmarks`"),
    ("target_mixed_forms", "expected `allocator`"),
    ("target_unsupported_option", "unsupported group option"),
];

fn compile_case(name: &str) -> Output {
    let fixture = compile_fixture();

    Command::new(env!("CARGO"))
        .args(["check", "--offline", "--quiet", "--manifest-path"])
        .arg(&fixture.manifest)
        .args(["--bin", name])
        .env("CARGO_TARGET_DIR", &fixture.target)
        .output()
        .unwrap()
}

struct CompileFixture {
    _directory: TempDir,
    manifest: PathBuf,
    target: PathBuf,
}

fn compile_fixture() -> &'static CompileFixture {
    static FIXTURE: OnceLock<CompileFixture> = OnceLock::new();

    FIXTURE.get_or_init(|| {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = crate_root.parent().and_then(Path::parent).unwrap();
        let workspace_root = workspace_root.to_string_lossy().replace('\\', "\\\\");
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("Cargo.toml");
        let contents = include_str!("compile_cases.toml").replace("../../../crates", &format!("{workspace_root}/crates"));
        fs::write(&manifest, contents).unwrap();
        let target = directory.path().join("target");
        CompileFixture {
            _directory: directory,
            manifest,
            target,
        }
    })
}

fn display_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[cfg_attr(miri, ignore)]
fn compile_pass_contracts() {
    for name in PASS_CASES {
        let output = compile_case(name);
        assert!(output.status.success(), "{name} failed to compile:\n{}", display_output(&output));
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn compile_fail_contracts() {
    for (name, expected) in FAIL_CASES {
        let output = compile_case(name);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{name} unexpectedly compiled");
        assert!(
            stderr.contains(expected),
            "{name} did not report {expected:?}:\n{}",
            display_output(&output)
        );
    }
}
