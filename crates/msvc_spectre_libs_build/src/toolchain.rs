// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The environment capabilities the build policy depends on.
//!
//! [`plan`](crate::plan::plan) is a decision function: given what was observed,
//! it says what the build script should do. Two of its inputs cannot be
//! captured up front, because which question gets asked depends on the answers
//! to the earlier ones -- whether a candidate directory exists, and where the
//! MSVC compiler lives. This module names those two capabilities as a trait so
//! that the policy takes them as a parameter instead of reaching for the real
//! machine, which is what makes the policy testable without an installed
//! toolchain.

use std::path::{Path, PathBuf};

/// The machine-dependent questions the build policy needs answered.
///
/// [`SystemToolchain`] is the real implementation. Tests substitute their own,
/// which is what lets the whole decision be exercised on any host.
pub trait Toolchain {
    /// Returns whether `path` names an existing directory.
    fn is_dir(&self, path: &Path) -> bool;

    /// Locates the MSVC C compiler for `target`, if one is installed.
    ///
    /// Returns the full path of `cl.exe`.
    fn find_cl_exe(&self, target: &str) -> Option<PathBuf>;
}

/// The [`Toolchain`] backed by the real filesystem and the Windows registry.
///
/// Registry discovery goes through `cc::windows_registry`, which is
/// target-aware: it can locate an installation for a Windows MSVC target even
/// from a host that is not the target architecture.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemToolchain;

impl Toolchain for SystemToolchain {
    #[cfg_attr(test, mutants::skip)] // Delegates straight to the filesystem; the policy that consults it is tested with a fake.
    #[cfg_attr(coverage_nightly, coverage(off))] // Pure I/O; the policy that consults it is covered through `plan`.
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    #[cfg_attr(test, mutants::skip)] // Delegates straight to the registry; the policy that consults it is tested with a fake.
    #[cfg_attr(coverage_nightly, coverage(off))] // Pure I/O; the policy that consults it is covered through `plan`.
    // #[gamma::skip(fn_value.none, tag = "adapter", reason = "This adapter queries the real Windows registry; the target, executable, found-path, and absent-path contracts are tested through injected find_cl_exe_with.")]
    fn find_cl_exe(&self, target: &str) -> Option<PathBuf> {
        // #[gamma::skip(call.replace_with_default, call_result.default, tag = "adapter", reason = "This call delegates to real registry discovery; deterministic found/absent behavior is exercised through the injected lookup seam.")]
        find_cl_exe_with(target, |target, name| {
            cc::windows_registry::find_tool(target, name).map(|tool| tool.path().to_path_buf())
        })
    }
}

fn find_cl_exe_with(target: &str, find: impl FnOnce(&str, &str) -> Option<PathBuf>) -> Option<PathBuf> {
    find(target, "cl.exe")
}

#[cfg(test)]
mod gamma_round2 {
    use std::path::PathBuf;

    use super::find_cl_exe_with;

    #[test]
    fn gamma_round2_compiler_lookup_preserves_target_executable_and_result() {
        let path = PathBuf::from(r"C:\controlled\cl.exe");
        assert_eq!(
            find_cl_exe_with("aarch64-pc-windows-msvc", |target, executable| {
                assert_eq!(target, "aarch64-pc-windows-msvc");
                assert_eq!(executable, "cl.exe");
                Some(path.clone())
            }),
            Some(path),
        );
        assert_eq!(
            find_cl_exe_with("missing-target", |_, executable| {
                assert_eq!(executable, "cl.exe");
                None
            }),
            None
        );
    }
}
