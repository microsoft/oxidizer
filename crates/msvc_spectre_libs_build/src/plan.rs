// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The build-script decision, expressed as a value.
//!
//! [`plan`] takes a description of everything that was observed -- the selected
//! target, the state of each environment variable that matters, and a
//! [`Toolchain`] to answer the two machine-dependent questions -- and returns
//! the [`Plan`] the build script should carry out. It reads no process
//! environment, prints nothing, and never exits, so the whole policy can be
//! exercised in-process from a table of cases rather than by spawning Cargo
//! against an installed MSVC toolchain.
//!
//! The build script of `msvc_spectre_libs` is the adapter around it: it fills
//! in a [`BuildEnvironment`] from the process environment, calls [`plan`], and
//! prints the resulting directives.

use std::env::{self, VarError};
use std::path::{Path, PathBuf};

use ohno::{AppError, app_err};

use crate::flags;
use crate::resolve::{self, SpectreArch};
use crate::toolchain::Toolchain;

/// The target-agnostic Spectre library directory override.
///
/// It applies to every target that has no target-specific override set; see
/// [`resolve::override_var_name`] for the preferred, target-suffixed spelling.
pub const GENERIC_LIB_DIR_VAR: &str = "MSVC_SPECTRE_LIB_DIR";

/// The variable an MSVC developer environment exports for the tools root.
///
/// A Visual Studio developer command prompt, and any shell that has run
/// `vcvars`, sets it to the MSVC build-tools directory -- exactly the directory
/// the Spectre libraries sit beneath.
pub const VC_TOOLS_INSTALL_DIR_VAR: &str = "VCToolsInstallDir";

/// The observed state of one environment variable.
///
/// The three states are kept apart deliberately. Folding [`Self::NotUnicode`]
/// into [`Self::Absent`] would silently skip a check, or silently fall back to
/// toolchain discovery, in exactly the case where an integrator did configure
/// something.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum EnvValue {
    /// The variable is not set.
    #[default]
    Absent,

    /// The variable is set, but its value is not valid Unicode.
    NotUnicode,

    /// The variable is set to the given value.
    Present(String),
}

impl EnvValue {
    /// Reads the current state of `name` from the process environment.
    ///
    /// This is the only part of this module that touches the environment; it
    /// exists so that the adapter does not have to repeat the three-way match.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Reads the process environment; the three-way outcome is exercised through `BuildEnvironment`.
    pub fn read(name: &str) -> Self {
        match env::var(name) {
            Ok(value) => Self::Present(value),
            Err(VarError::NotPresent) => Self::Absent,
            Err(VarError::NotUnicode(_)) => Self::NotUnicode,
        }
    }
}

/// Everything the build script observed before deciding what to do.
///
/// The target fields carry the values Cargo passes in `TARGET` and
/// `CARGO_CFG_TARGET_OS` / `_ENV` / `_ARCH`. They describe the target being
/// built, not the host running the build script, which is why the policy gates
/// on them rather than on a compile-time `cfg!`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuildEnvironment {
    /// The selected target triple, from `TARGET`.
    pub target: String,

    /// The target operating system, from `CARGO_CFG_TARGET_OS`.
    pub target_os: String,

    /// The target C environment, from `CARGO_CFG_TARGET_ENV`.
    pub target_env: String,

    /// The target architecture, from `CARGO_CFG_TARGET_ARCH`.
    pub target_arch: String,

    /// The target-suffixed library directory override.
    pub target_lib_dir: EnvValue,

    /// The target-agnostic [`GENERIC_LIB_DIR_VAR`] override.
    pub generic_lib_dir: EnvValue,

    /// The [`VC_TOOLS_INSTALL_DIR_VAR`] developer-environment variable.
    pub vc_tools_install_dir: EnvValue,

    /// The target-suffixed required-linker-argument list.
    pub target_required_link_args: EnvValue,

    /// The target-agnostic [`flags::REQUIRED_LINK_ARGS_VAR`] list.
    pub generic_required_link_args: EnvValue,

    /// The flags Cargo reports it will pass to `rustc`.
    pub encoded_rustflags: EnvValue,
}

impl BuildEnvironment {
    /// Captures the build environment from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when `TARGET`, `CARGO_CFG_TARGET_OS`,
    /// `CARGO_CFG_TARGET_ENV`, or `CARGO_CFG_TARGET_ARCH` is missing or not
    /// valid Unicode. Cargo sets all four for every build script, so their
    /// absence means this is not running as one and no useful decision can be
    /// made.
    #[cfg_attr(test, mutants::skip)] // Pure capture of the process environment; the decisions it feeds are tested directly.
    pub fn from_env() -> Result<Self, AppError> {
        let target = required_cargo_var("TARGET")?;

        Ok(Self {
            target_lib_dir: EnvValue::read(&resolve::override_var_name(&target)),
            generic_lib_dir: EnvValue::read(GENERIC_LIB_DIR_VAR),
            vc_tools_install_dir: EnvValue::read(VC_TOOLS_INSTALL_DIR_VAR),
            target_required_link_args: EnvValue::read(&flags::required_link_args_var_name(&target)),
            generic_required_link_args: EnvValue::read(flags::REQUIRED_LINK_ARGS_VAR),
            encoded_rustflags: EnvValue::read(flags::CARGO_ENCODED_RUSTFLAGS_VAR),
            target_os: required_cargo_var("CARGO_CFG_TARGET_OS")?,
            target_env: required_cargo_var("CARGO_CFG_TARGET_ENV")?,
            target_arch: required_cargo_var("CARGO_CFG_TARGET_ARCH")?,
            target,
        })
    }
}

/// Reads a variable Cargo guarantees for build scripts.
///
/// # Errors
///
/// Returns an error when the variable is missing or not valid Unicode.
#[cfg_attr(test, mutants::skip)] // Reads the process environment, which the tests deliberately do not depend on.
fn required_cargo_var(name: &str) -> Result<String, AppError> {
    env::var(name).map_err(|error| match error {
        VarError::NotPresent => app_err!("`{name}` is not set; this code must run as a cargo build script"),
        VarError::NotUnicode(_) => app_err!("`{name}` is not valid Unicode"),
    })
}

/// What the build script should do, as data.
///
/// Every field is ordered as the corresponding directives should be printed.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Plan {
    /// Variable names to emit as `cargo:rerun-if-env-changed=`.
    pub rerun_if_env_changed: Vec<String>,

    /// Directories to emit as `cargo:rustc-link-search=native=`.
    ///
    /// Guaranteed to be valid UTF-8 and to have existed at planning time.
    pub link_search: Vec<String>,

    /// Problems to report, as `cargo:warning=` or as a hard failure.
    pub diagnostics: Vec<AppError>,
}

impl Plan {
    /// Builds a plan that does nothing but report `error`.
    ///
    /// Used when the environment could not even be captured, so no decision
    /// was reachable.
    #[must_use]
    pub fn reporting(error: AppError) -> Self {
        Self {
            diagnostics: vec![error],
            ..Self::default()
        }
    }

    /// Returns whether anything went wrong.
    ///
    /// The `error` feature of `msvc_spectre_libs` turns this into a failed
    /// build; without it the diagnostics are warnings and the build proceeds.
    #[must_use]
    pub fn failed(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

/// Decides what the build script should do.
///
/// Both steps -- resolving the library search path and verifying the required
/// linker arguments -- are always evaluated, so a single build surfaces every
/// problem instead of one per iteration.
///
/// For a target that is not Windows MSVC the plan is empty apart from the
/// [`GENERIC_LIB_DIR_VAR`] rerun registration, which is emitted for every
/// target so that setting the override on a previously-skipped target takes
/// effect without a clean build.
#[must_use]
pub fn plan(environment: &BuildEnvironment, toolchain: &dyn Toolchain) -> Plan {
    let mut plan = Plan::default();
    plan.rerun_if_env_changed.push(GENERIC_LIB_DIR_VAR.to_owned());

    // A build script runs on the *host*, so a compile-time `cfg!(target_os)`
    // would describe the machine compiling this code rather than the target
    // Cargo selected. Gate on the target Cargo reported instead: that reaches
    // resolution for every Windows MSVC target regardless of host, and no-ops
    // everywhere else.
    if environment.target_os != "windows" || environment.target_env != "msvc" {
        return plan;
    }

    if let Err(error) = plan_link_search(environment, toolchain, &mut plan) {
        plan.diagnostics.push(error);
    }

    if let Err(error) = plan_required_link_args(environment, &mut plan) {
        plan.diagnostics.push(error);
    }

    plan
}

/// Resolves the Spectre library directory and records it in `plan`.
///
/// # Errors
///
/// Returns a human-readable error when the directory cannot be located:
/// a missing or unreadable override, an architecture with no mitigated CRT, a
/// `cl.exe` that cannot be found, or a toolchain without the Spectre libraries.
fn plan_link_search(environment: &BuildEnvironment, toolchain: &dyn Toolchain, plan: &mut Plan) -> Result<(), AppError> {
    let override_var = resolve::override_var_name(&environment.target);
    plan.rerun_if_env_changed.push(override_var.clone());

    // 1. An explicit build-system override wins over discovery, and never
    //    falls through to it: an integrator who named a directory wants that
    //    directory or a diagnostic, not a silent substitute. The variable that
    //    supplied the value is remembered so the diagnostic names the one that
    //    is actually set rather than the one that merely takes precedence.
    let overrides = [
        (override_var.as_str(), &environment.target_lib_dir),
        (GENERIC_LIB_DIR_VAR, &environment.generic_lib_dir),
    ];

    for (source_var, value) in overrides {
        match value {
            EnvValue::Absent => {}
            EnvValue::NotUnicode => return Err(not_unicode(source_var, "used as a library directory")),
            EnvValue::Present(dir) => {
                let dir = PathBuf::from(dir);
                return if push_link_search(plan, toolchain, &dir) {
                    Ok(())
                } else {
                    Err(app_err!(
                        "the Spectre library directory `{}` provided via `{source_var}` does not exist",
                        dir.display()
                    ))
                };
            }
        }
    }

    let target_arch = &environment.target_arch;
    let Some(arch) = SpectreArch::from_target_arch(target_arch) else {
        return Err(app_err!(
            "target architecture `{target_arch}` has no known Spectre-mitigated CRT; set `{override_var}` to override"
        ));
    };

    // 2. Prefer the toolchain root exported by the build environment. It points
    //    straight at the MSVC build tools, so the Spectre libraries sit directly
    //    beneath it -- no parent-directory climbing relative to `cl.exe`. A root
    //    that does not carry them is not an error on its own; registry discovery
    //    may still find an installation that does.
    plan.rerun_if_env_changed.push(VC_TOOLS_INSTALL_DIR_VAR.to_owned());
    if let EnvValue::Present(vc_tools) = &environment.vc_tools_install_dir {
        let dir = resolve::spectre_lib_dir(Path::new(vc_tools), arch);
        if push_link_search(plan, toolchain, &dir) {
            return Ok(());
        }
    }

    // 3. Fall back to discovering the toolchain through the Windows registry.
    let target = &environment.target;
    let Some(cl_exe) = toolchain.find_cl_exe(target) else {
        return Err(app_err!(
            "could not locate `cl.exe` for target `{target}`; set `{override_var}` to point at the Spectre library directory"
        ));
    };

    let Some(root) = toolchain_root(&cl_exe) else {
        return Err(app_err!(
            "could not derive the toolchain root from `{}`; set `{override_var}` to point at the Spectre library directory",
            cl_exe.display()
        ));
    };

    let spectre_libs = resolve::spectre_lib_dir(root, arch);
    if push_link_search(plan, toolchain, &spectre_libs) {
        Ok(())
    } else {
        Err(app_err!(
            "no Spectre-mitigated libraries were found at `{}`; modify the Visual Studio installation to add them, or set \
             `{override_var}`",
            spectre_libs.display()
        ))
    }
}

/// Verifies that the linker arguments this crate cannot propagate reached `rustc`.
///
/// A `cargo:rustc-link-arg` applies only to the emitting package's own
/// artifacts, so these arguments have to come from `.cargo/config.toml` or
/// `RUSTFLAGS`. Since a `RUSTFLAGS` environment variable *replaces* the
/// `target.<triple>.rustflags` table rather than merging with it, an ambient
/// `RUSTFLAGS` silently drops them; this check makes that visible.
///
/// Does nothing when no requirement is configured, which is the default.
///
/// # Errors
///
/// Returns a human-readable error listing the arguments that are missing, or
/// naming a configured variable whose value could not be read.
fn plan_required_link_args(environment: &BuildEnvironment, plan: &mut Plan) -> Result<(), AppError> {
    let target_var = flags::required_link_args_var_name(&environment.target);
    let requirements = [
        (target_var.as_str(), &environment.target_required_link_args),
        (flags::REQUIRED_LINK_ARGS_VAR, &environment.generic_required_link_args),
    ];

    let mut configured = None;
    for (source_var, value) in requirements {
        // Registered before inspecting, so that setting a variable that is
        // currently unset re-runs the script. The loop stops at the first
        // variable that carries a value, because the ones after it are ignored
        // and a change to them cannot alter the outcome.
        plan.rerun_if_env_changed.push(source_var.to_owned());
        match value {
            EnvValue::Absent => {}
            EnvValue::NotUnicode => return Err(not_unicode(source_var, "checked")),
            EnvValue::Present(value) => {
                configured = Some((source_var.to_owned(), value.clone()));
                break;
            }
        }
    }

    let Some((source_var, configured)) = configured else {
        return Ok(());
    };

    let required = flags::required_link_args(&configured);
    if required.is_empty() {
        return Ok(());
    }

    plan.rerun_if_env_changed.push(flags::CARGO_ENCODED_RUSTFLAGS_VAR.to_owned());
    let encoded = match &environment.encoded_rustflags {
        EnvValue::Present(encoded) => encoded.as_str(),
        // Absent means this Cargo does not report the final flags, so there is
        // nothing to verify and no basis for a diagnostic.
        EnvValue::Absent => return Ok(()),
        EnvValue::NotUnicode => return Err(not_unicode(flags::CARGO_ENCODED_RUSTFLAGS_VAR, "checked")),
    };

    let missing = flags::missing_required_link_args(encoded, &required);
    if missing.is_empty() {
        return Ok(());
    }

    // Spell out the `-Clink-arg=` form: these are linker arguments, so the bare
    // token is not something rustc accepts on its own.
    let as_rustflags = missing.iter().map(|arg| format!("-Clink-arg={arg}")).collect::<Vec<_>>().join(" ");

    Err(app_err!(
        "linker argument(s) `{}` required by `{source_var}` did not reach rustc. Add them to `[target.<triple>] rustflags` in \
         `.cargo/config.toml` as `{as_rustflags}`. If a `RUSTFLAGS` environment variable is set, note that it REPLACES the config \
         `rustflags` rather than merging with it, so the configured flags are dropped; add `{as_rustflags}` to `RUSTFLAGS` as well, or \
         unset it.",
        missing.join("`, `")
    ))
}

/// Records a link-search directory when it exists and can be written out.
///
/// Returns whether it was recorded. A `cargo:` directive is a UTF-8 text line,
/// so a path that is not valid Unicode is rejected rather than converted with
/// replacement characters, which would point the linker at a directory other
/// than the one that was checked.
///
/// An existing directory is always recorded, even if the same directory is
/// already reachable through `LIB`: an explicit `-L` search path is consulted
/// before `LIB` and in the order given, so suppressing it could leave an
/// ordinary CRT directory ahead of the mitigated one. A duplicate search path
/// is harmless to the linker.
fn push_link_search(plan: &mut Plan, toolchain: &dyn Toolchain, dir: &Path) -> bool {
    let Some(text) = dir.to_str() else {
        return false;
    };

    if !toolchain.is_dir(dir) {
        return false;
    }

    plan.link_search.push(text.to_owned());
    true
}

/// Derives the MSVC toolchain root from the path of a compiler executable.
fn toolchain_root(cl_exe: &Path) -> Option<&Path> {
    // `<root>\bin\Host<arch>\<arch>\cl.exe`: drop the file name, the target and
    // host compiler directories, and `bin`.
    cl_exe.parent()?.parent()?.parent()?.parent()
}

/// Builds the diagnostic for a configured variable that cannot be read.
///
/// `purpose` completes the sentence "so it could not be ...".
fn not_unicode(name: &str, purpose: &str) -> AppError {
    app_err!("the environment variable `{name}` is set but its value is not valid Unicode, so it could not be {purpose}")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::{BuildEnvironment, EnvValue, GENERIC_LIB_DIR_VAR, Plan, VC_TOOLS_INSTALL_DIR_VAR, app_err, plan};
    use crate::resolve::{SpectreArch, spectre_lib_dir};
    use crate::toolchain::Toolchain;

    const TARGET: &str = "x86_64-pc-windows-msvc";
    const TARGET_LIB_DIR_VAR: &str = "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc";
    const TARGET_REQUIRED_VAR: &str = "MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc";
    const VC_TOOLS: &str = "C:/VC/Tools/MSVC/14.40";
    const REGISTRY_ROOT: &str = "C:/Registry/VC/Tools/MSVC/14.40";
    const REGISTRY_CL: &str = "C:/Registry/VC/Tools/MSVC/14.40/bin/Hostx64/x64/cl.exe";

    /// The `lib\spectre\x64` directory beneath `root`, spelled exactly as the
    /// policy builds it -- the separator is the host's, not the one in `root`.
    fn spectre_dir(root: &str) -> String {
        spectre_lib_dir(Path::new(root), SpectreArch::X64)
            .to_str()
            .expect("test paths are UTF-8")
            .to_owned()
    }

    /// A [`Toolchain`] whose answers are fixed by the test.
    #[derive(Default)]
    struct FakeToolchain {
        directories: BTreeSet<PathBuf>,
        cl_exe: Option<PathBuf>,
    }

    impl FakeToolchain {
        fn with_dirs(dirs: &[String]) -> Self {
            Self {
                directories: dirs.iter().map(PathBuf::from).collect(),
                cl_exe: None,
            }
        }

        fn with_cl_exe(mut self, cl_exe: &str) -> Self {
            self.cl_exe = Some(PathBuf::from(cl_exe));
            self
        }
    }

    impl Toolchain for FakeToolchain {
        fn is_dir(&self, path: &Path) -> bool {
            self.directories.contains(path)
        }

        fn find_cl_exe(&self, _target: &str) -> Option<PathBuf> {
            self.cl_exe.clone()
        }
    }

    /// A Windows MSVC environment with nothing configured.
    fn windows_env() -> BuildEnvironment {
        BuildEnvironment {
            target: TARGET.to_owned(),
            target_os: "windows".to_owned(),
            target_env: "msvc".to_owned(),
            target_arch: "x86_64".to_owned(),
            ..BuildEnvironment::default()
        }
    }

    fn diagnostics(plan: &Plan) -> Vec<String> {
        plan.diagnostics.iter().map(ToString::to_string).collect()
    }

    fn assert_reports(plan: &Plan, expected: &str) {
        let reported = diagnostics(plan);
        assert!(
            reported.iter().any(|message| message.contains(expected)),
            "expected a diagnostic containing {expected:?}, got {reported:?}"
        );
    }

    #[test]
    fn skips_every_target_that_is_not_windows_msvc() {
        let cases = [("linux", "gnu"), ("windows", "gnu"), ("macos", ""), ("", "")];

        for (target_os, target_env) in cases {
            let environment = BuildEnvironment {
                target_os: target_os.to_owned(),
                target_env: target_env.to_owned(),
                ..windows_env()
            };
            let plan = plan(&environment, &FakeToolchain::default());

            assert!(plan.link_search.is_empty(), "{target_os}-{target_env} emitted a search path");
            assert!(!plan.failed(), "{target_os}-{target_env} reported {:?}", diagnostics(&plan));
            // The target-agnostic override is still registered, so setting it
            // later re-runs the script even on a skipped target.
            assert_eq!(plan.rerun_if_env_changed, vec![GENERIC_LIB_DIR_VAR.to_owned()]);
        }
    }

    #[test]
    fn prefers_the_target_specific_override_over_every_other_source() {
        let environment = BuildEnvironment {
            target_lib_dir: EnvValue::Present("C:/feed/spectre".to_owned()),
            generic_lib_dir: EnvValue::Present("C:/generic/spectre".to_owned()),
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            ..windows_env()
        };
        let toolchain = FakeToolchain::with_dirs(&["C:/feed/spectre".to_owned(), "C:/generic/spectre".to_owned(), spectre_dir(VC_TOOLS)]);

        let plan = plan(&environment, &toolchain);

        assert_eq!(plan.link_search, vec!["C:/feed/spectre".to_owned()]);
        assert!(!plan.failed());
    }

    #[test]
    fn falls_back_to_the_target_agnostic_override() {
        let environment = BuildEnvironment {
            generic_lib_dir: EnvValue::Present("C:/generic/spectre".to_owned()),
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            ..windows_env()
        };
        let toolchain = FakeToolchain::with_dirs(&["C:/generic/spectre".to_owned(), spectre_dir(VC_TOOLS)]);

        let plan = plan(&environment, &toolchain);

        assert_eq!(plan.link_search, vec!["C:/generic/spectre".to_owned()]);
    }

    #[test]
    fn reports_an_override_directory_that_does_not_exist_without_falling_back() {
        let environment = BuildEnvironment {
            target_lib_dir: EnvValue::Present("C:/missing".to_owned()),
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            ..windows_env()
        };
        // The discovery sources would have succeeded; the override still wins.
        let toolchain = FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS), spectre_dir(REGISTRY_ROOT)]).with_cl_exe(REGISTRY_CL);

        let plan = plan(&environment, &toolchain);

        assert!(plan.link_search.is_empty());
        assert_reports(
            &plan,
            "`C:/missing` provided via `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc` does not exist",
        );
    }

    #[test]
    fn reports_an_override_that_is_not_valid_unicode() {
        for (source_var, environment) in [
            (
                TARGET_LIB_DIR_VAR,
                BuildEnvironment {
                    target_lib_dir: EnvValue::NotUnicode,
                    ..windows_env()
                },
            ),
            (
                GENERIC_LIB_DIR_VAR,
                BuildEnvironment {
                    generic_lib_dir: EnvValue::NotUnicode,
                    ..windows_env()
                },
            ),
        ] {
            let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

            assert!(plan.link_search.is_empty());
            assert_reports(&plan, &format!("`{source_var}` is set but its value is not valid Unicode"));
        }
    }

    #[test]
    fn uses_the_developer_environment_toolchain_root() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            ..windows_env()
        };
        let toolchain = FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]);

        let plan = plan(&environment, &toolchain);

        assert_eq!(plan.link_search, vec![spectre_dir(VC_TOOLS)]);
        assert!(plan.rerun_if_env_changed.contains(&VC_TOOLS_INSTALL_DIR_VAR.to_owned()));
    }

    #[test]
    fn falls_through_a_toolchain_root_without_spectre_libraries() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            ..windows_env()
        };
        // `VCToolsInstallDir` is set but carries no mitigated libraries, so
        // registry discovery still gets a chance.
        let toolchain = FakeToolchain::with_dirs(&[spectre_dir(REGISTRY_ROOT)]).with_cl_exe(REGISTRY_CL);

        let plan = plan(&environment, &toolchain);

        assert_eq!(plan.link_search, vec![spectre_dir(REGISTRY_ROOT)]);
        assert!(!plan.failed());
    }

    #[test]
    fn discovers_the_toolchain_through_the_registry() {
        let toolchain = FakeToolchain::with_dirs(&[spectre_dir(REGISTRY_ROOT)]).with_cl_exe(REGISTRY_CL);

        let plan = plan(&windows_env(), &toolchain);

        assert_eq!(plan.link_search, vec![spectre_dir(REGISTRY_ROOT)]);
    }

    #[test]
    fn reports_a_target_architecture_without_mitigated_libraries() {
        let environment = BuildEnvironment {
            target_arch: "riscv64".to_owned(),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::default());

        assert_reports(&plan, "target architecture `riscv64` has no known Spectre-mitigated CRT");
    }

    #[test]
    fn reports_a_toolchain_that_cannot_be_discovered() {
        let plan = plan(&windows_env(), &FakeToolchain::default());

        assert_reports(&plan, "could not locate `cl.exe` for target `x86_64-pc-windows-msvc`");
    }

    #[test]
    fn reports_a_compiler_path_with_no_toolchain_root() {
        let toolchain = FakeToolchain::default().with_cl_exe("cl.exe");

        let plan = plan(&windows_env(), &toolchain);

        assert_reports(&plan, "could not derive the toolchain root from `cl.exe`");
    }

    #[test]
    fn reports_a_toolchain_without_the_spectre_libraries_installed() {
        let toolchain = FakeToolchain::default().with_cl_exe(REGISTRY_CL);

        let plan = plan(&windows_env(), &toolchain);

        assert_reports(
            &plan,
            &format!("no Spectre-mitigated libraries were found at `{}`", spectre_dir(REGISTRY_ROOT)),
        );
    }

    #[test]
    fn verifies_nothing_when_no_requirement_is_configured() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            encoded_rustflags: EnvValue::Present(String::new()),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

        assert!(!plan.failed());
        assert!(!plan.rerun_if_env_changed.contains(&"CARGO_ENCODED_RUSTFLAGS".to_owned()));
    }

    #[test]
    fn accepts_a_requirement_that_reached_rustc() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            target_required_link_args: EnvValue::Present("/CETCOMPAT".to_owned()),
            encoded_rustflags: EnvValue::Present("-C\u{1f}link-arg=/CETCOMPAT".to_owned()),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

        assert!(!plan.failed(), "{:?}", diagnostics(&plan));
        assert!(plan.rerun_if_env_changed.contains(&"CARGO_ENCODED_RUSTFLAGS".to_owned()));
    }

    #[test]
    fn reports_a_requirement_that_did_not_reach_rustc() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            target_required_link_args: EnvValue::Present("/CETCOMPAT;/guard:ehcont".to_owned()),
            encoded_rustflags: EnvValue::Present("-Clink-arg=/CETCOMPAT".to_owned()),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

        assert_reports(
            &plan,
            "linker argument(s) `/guard:ehcont` required by `MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc`",
        );
        assert_reports(&plan, "-Clink-arg=/guard:ehcont");
        // The search path was still resolved: one build reports every problem.
        assert_eq!(plan.link_search, vec![spectre_dir(VC_TOOLS)]);
    }

    #[test]
    fn prefers_the_target_specific_requirement_over_the_generic_one() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            target_required_link_args: EnvValue::Present("/CETCOMPAT".to_owned()),
            generic_required_link_args: EnvValue::Present("/guard:ehcont".to_owned()),
            encoded_rustflags: EnvValue::Present("-Clink-arg=/CETCOMPAT".to_owned()),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

        assert!(!plan.failed(), "{:?}", diagnostics(&plan));
        // The generic variable is ignored once the target-specific one carries
        // a value, so it is not registered for rerun either.
        assert!(!plan.rerun_if_env_changed.contains(&"MSVC_SPECTRE_REQUIRED_LINK_ARGS".to_owned()));
        assert!(plan.rerun_if_env_changed.contains(&TARGET_REQUIRED_VAR.to_owned()));
    }

    #[test]
    fn skips_verification_when_cargo_does_not_report_the_flags() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            target_required_link_args: EnvValue::Present("/CETCOMPAT".to_owned()),
            encoded_rustflags: EnvValue::Absent,
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

        assert!(!plan.failed(), "{:?}", diagnostics(&plan));
    }

    #[test]
    fn ignores_a_requirement_list_with_no_entries() {
        let environment = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            target_required_link_args: EnvValue::Present(" ; ".to_owned()),
            encoded_rustflags: EnvValue::Present(String::new()),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

        assert!(!plan.failed(), "{:?}", diagnostics(&plan));
    }

    #[test]
    fn reports_unreadable_requirement_and_flag_variables() {
        let base = BuildEnvironment {
            vc_tools_install_dir: EnvValue::Present(VC_TOOLS.to_owned()),
            ..windows_env()
        };

        let cases = [
            (
                TARGET_REQUIRED_VAR,
                BuildEnvironment {
                    target_required_link_args: EnvValue::NotUnicode,
                    ..base.clone()
                },
            ),
            (
                "MSVC_SPECTRE_REQUIRED_LINK_ARGS",
                BuildEnvironment {
                    generic_required_link_args: EnvValue::NotUnicode,
                    ..base.clone()
                },
            ),
            (
                "CARGO_ENCODED_RUSTFLAGS",
                BuildEnvironment {
                    target_required_link_args: EnvValue::Present("/CETCOMPAT".to_owned()),
                    encoded_rustflags: EnvValue::NotUnicode,
                    ..base
                },
            ),
        ];

        for (source_var, environment) in cases {
            let plan = plan(&environment, &FakeToolchain::with_dirs(&[spectre_dir(VC_TOOLS)]));

            assert_reports(&plan, &format!("`{source_var}` is set but its value is not valid Unicode"));
            // A verification failure never suppresses the search path.
            assert_eq!(plan.link_search, vec![spectre_dir(VC_TOOLS)]);
        }
    }

    #[test]
    fn reports_an_environment_that_could_not_be_captured() {
        let plan = Plan::reporting(app_err!("TARGET is not set"));

        assert!(plan.failed());
        assert!(plan.link_search.is_empty());
        assert!(plan.rerun_if_env_changed.is_empty());
        assert_reports(&plan, "TARGET is not set");
    }

    #[test]
    fn reports_both_failures_from_a_single_build() {
        let environment = BuildEnvironment {
            target_lib_dir: EnvValue::Present("C:/missing".to_owned()),
            target_required_link_args: EnvValue::Present("/CETCOMPAT".to_owned()),
            encoded_rustflags: EnvValue::Present(String::new()),
            ..windows_env()
        };

        let plan = plan(&environment, &FakeToolchain::default());

        assert_eq!(plan.diagnostics.len(), 2, "{:?}", diagnostics(&plan));
        assert!(plan.failed());
    }
}
