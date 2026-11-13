// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Injected build policy contracts and read-only metadata of existing crate files.

use std::path::{Path, PathBuf};

use msvc_spectre_libs_build::plan::{BuildEnvironment, EnvValue, plan};
use msvc_spectre_libs_build::toolchain::Toolchain;

struct FakeToolchain {
    exists: bool,
    compiler: Option<PathBuf>,
}

impl Toolchain for FakeToolchain {
    fn is_dir(&self, _: &Path) -> bool {
        self.exists
    }

    fn find_cl_exe(&self, _: &str) -> Option<PathBuf> {
        self.compiler.clone()
    }
}

fn environment() -> BuildEnvironment {
    BuildEnvironment {
        target: String::from("x86_64-pc-windows-msvc"),
        target_os: String::from("windows"),
        target_env: String::from("msvc"),
        target_arch: String::from("x86_64"),
        target_lib_dir: EnvValue::Present(String::from(r"C:\spectre")),
        ..BuildEnvironment::default()
    }
}

#[test]
fn gamma_survivor_msvc_environment_on_nonwindows_target_skips_resolution() {
    let mut environment = environment();
    environment.target_os = String::from("linux");
    let plan = plan(
        &environment,
        &FakeToolchain {
            exists: false,
            compiler: None,
        },
    );
    assert!(!plan.failed());
    assert!(plan.link_search.is_empty());
    assert_eq!(plan.rerun_if_env_changed, ["MSVC_SPECTRE_LIB_DIR"]);
}

#[test]
fn gamma_survivor_plan_registers_target_override_and_required_flags_without_reordering() {
    let mut environment = environment();
    environment.target_required_link_args = EnvValue::Present(String::from("/CETCOMPAT"));
    environment.encoded_rustflags = EnvValue::Present(String::from("-Clink-arg=/CETCOMPAT"));
    let plan = plan(
        &environment,
        &FakeToolchain {
            exists: true,
            compiler: None,
        },
    );
    assert!(!plan.failed());
    assert_eq!(plan.link_search, [r"C:\spectre"]);
    assert_eq!(
        plan.rerun_if_env_changed,
        [
            "MSVC_SPECTRE_LIB_DIR",
            "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc",
            "MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc",
            "CARGO_ENCODED_RUSTFLAGS",
        ]
    );
}

#[test]
fn gamma_survivor_plan_names_unreadable_values_and_the_operation_that_cannot_run() {
    let mut library = environment();
    library.target_lib_dir = EnvValue::NotUnicode;
    let mut requirements = environment();
    requirements.target_required_link_args = EnvValue::NotUnicode;
    let mut flags = environment();
    flags.target_required_link_args = EnvValue::Present(String::from("/CETCOMPAT"));
    flags.encoded_rustflags = EnvValue::NotUnicode;
    for (environment, expected) in [
        (
            library,
            "the environment variable `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc` is set but its value is not valid Unicode, so it could not be used as a library directory",
        ),
        (
            requirements,
            "the environment variable `MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc` is set but its value is not valid Unicode, so it could not be checked",
        ),
        (
            flags,
            "the environment variable `CARGO_ENCODED_RUSTFLAGS` is set but its value is not valid Unicode, so it could not be checked",
        ),
    ] {
        let plan = plan(
            &environment,
            &FakeToolchain {
                exists: true,
                compiler: None,
            },
        );
        assert!(plan.failed());
        assert_eq!(plan.warnings(), [expected]);
    }
}

#[test]
fn gamma_survivor_plan_distinguishes_absent_flags_from_unreadable_flags() {
    let mut environment = environment();
    environment.target_required_link_args = EnvValue::Present(String::from("/CETCOMPAT"));
    let plan = plan(
        &environment,
        &FakeToolchain {
            exists: true,
            compiler: None,
        },
    );
    assert!(!plan.failed());
    assert_eq!(plan.link_search, [r"C:\spectre"]);
}

#[test]
fn gamma_survivor_plan_rejects_short_compiler_paths_and_sanitizes_each_line_break() {
    for path in [r"cl.exe", r"bin\cl.exe", r"Hostx64\x64\cl.exe"] {
        let mut environment = environment();
        environment.target_lib_dir = EnvValue::Absent;
        let plan = plan(
            &environment,
            &FakeToolchain {
                exists: false,
                compiler: Some(PathBuf::from(path)),
            },
        );
        assert!(plan.failed());
        assert!(plan.warnings()[0].contains("could not derive the toolchain root"));
        assert!(plan.link_search.is_empty());
    }
    for separator in ["\r", "\n"] {
        let mut environment = environment();
        environment.target_lib_dir = EnvValue::Present(format!("C:\\first{separator}second"));
        let plan = plan(
            &environment,
            &FakeToolchain {
                exists: true,
                compiler: None,
            },
        );
        assert!(plan.failed());
        assert!(plan.link_search.is_empty());
        let warning = &plan.warnings()[0];
        assert!(!warning.contains(['\r', '\n']));
        assert!(warning.contains("contains a line break, so it could not be given to the linker"));
        assert!(warning.contains("C:\\first second"));
    }
}

#[test]
fn gamma_survivor_nonlink_codegen_flags_do_not_satisfy_required_linker_args() {
    let mut environment = environment();
    environment.target_required_link_args = EnvValue::Present("/CETCOMPAT;/guard:cf".to_owned());
    environment.encoded_rustflags = EnvValue::Present("-Cembed-bitcode=/CETCOMPAT\u{1f}-Csymbol-mangling-version=/guard:cf".to_owned());
    let result = plan(
        &environment,
        &FakeToolchain {
            exists: true,
            compiler: None,
        },
    );
    assert!(result.failed());
    assert_eq!(result.warnings().len(), 1);
    assert!(
        result.warnings()[0].contains("as `-Clink-arg=/CETCOMPAT -Clink-arg=/guard:cf`"),
        "{:?}",
        result.warnings()
    );
}

#[test]
fn gamma_survivor_empty_required_flags_skip_unreadable_final_flags_without_registration() {
    let mut environment = environment();
    environment.target_required_link_args = EnvValue::Present(String::new());
    environment.encoded_rustflags = EnvValue::NotUnicode;
    let result = plan(
        &environment,
        &FakeToolchain {
            exists: true,
            compiler: None,
        },
    );
    assert!(!result.failed());
    assert!(!result.rerun_if_env_changed.iter().any(|name| name == "CARGO_ENCODED_RUSTFLAGS"));
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover filesystem probing; fake toolchains cover planning under Miri")]
fn gamma_survivor_system_directory_probe_distinguishes_existing_directory_from_file() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let toolchain = msvc_spectre_libs_build::toolchain::SystemToolchain;
    assert!(toolchain.is_dir(directory));
    assert!(!toolchain.is_dir(&directory.join("Cargo.toml")));
}
