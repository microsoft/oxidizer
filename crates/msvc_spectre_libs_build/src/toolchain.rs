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
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn find_cl_exe(&self, target: &str) -> Option<PathBuf> {
        cc::windows_registry::find_tool(target, "cl.exe").map(|tool| tool.path().to_path_buf())
    }
}
