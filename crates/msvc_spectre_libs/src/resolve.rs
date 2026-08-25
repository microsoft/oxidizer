// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pure helpers for locating the Spectre-mitigated MSVC CRT libraries.
//!
//! These functions perform no I/O, so they can be unit tested and reused both
//! by this crate's build script and by build-system integrators that want to
//! compute the override variable names themselves.

use std::fmt;
use std::path::{Path, PathBuf};

/// Architecture of a set of Spectre-mitigated MSVC CRT libraries.
///
/// A value of this type is only produced by [`SpectreArch::from_target_arch`],
/// so it can only name an architecture for which the MSVC toolchain ships
/// mitigated libraries. That makes a raw Cargo architecture string
/// unrepresentable once validation has succeeded, and lets
/// [`spectre_lib_dir`] rely on the directory component being a real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SpectreArch {
    /// 64-bit x86.
    X64,

    /// 32-bit x86.
    X86,

    /// 64-bit Arm, which also covers `Arm64EC`.
    Arm64,

    /// 32-bit Arm.
    Arm,
}

impl SpectreArch {
    /// Maps a Rust target architecture to the matching Spectre architecture.
    ///
    /// `target_arch` is the value of the `CARGO_CFG_TARGET_ARCH` build-script
    /// environment variable. Returns [`None`] for architectures that have no
    /// Spectre-mitigated CRT.
    ///
    /// # Examples
    ///
    /// ```
    /// use msvc_spectre_libs::resolve::SpectreArch;
    ///
    /// assert_eq!(
    ///     SpectreArch::from_target_arch("x86_64"),
    ///     Some(SpectreArch::X64)
    /// );
    /// // Arm64EC objects ship inside the 64-bit Arm libraries.
    /// assert_eq!(
    ///     SpectreArch::from_target_arch("arm64ec"),
    ///     Some(SpectreArch::Arm64)
    /// );
    /// assert_eq!(SpectreArch::from_target_arch("riscv64"), None);
    /// ```
    #[must_use]
    pub fn from_target_arch(target_arch: &str) -> Option<Self> {
        Some(match target_arch {
            "x86_64" => Self::X64,
            "x86" => Self::X86,
            // The Arm64EC Spectre directory ships no libraries of its own; the
            // 64-bit Arm libraries contain both kinds of object.
            "aarch64" | "arm64ec" => Self::Arm64,
            "arm" => Self::Arm,
            _ => return None,
        })
    }

    /// Returns the `lib\spectre\<arch>` subdirectory name for this
    /// architecture.
    ///
    /// # Examples
    ///
    /// ```
    /// use msvc_spectre_libs::resolve::SpectreArch;
    ///
    /// assert_eq!(SpectreArch::X64.dir_name(), "x64");
    /// ```
    #[must_use]
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::X86 => "x86",
            Self::Arm64 => "arm64",
            // 32-bit Arm has its own directory alongside the other
            // architectures in the MSVC toolchain layout.
            Self::Arm => "arm",
        }
    }
}

impl fmt::Display for SpectreArch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dir_name())
    }
}

/// Computes the target-specific override environment variable name.
///
/// The name is formed by replacing every `-` in the target triple with `_` and
/// prefixing `MSVC_SPECTRE_LIB_DIR_`. Setting this variable, or the
/// target-agnostic `MSVC_SPECTRE_LIB_DIR`, to an existing directory makes the
/// build script use that directory verbatim instead of probing the toolchain.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs::resolve::override_var_name;
///
/// assert_eq!(
///     override_var_name("x86_64-pc-windows-msvc"),
///     "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc"
/// );
/// ```
#[must_use]
pub fn override_var_name(target: &str) -> String {
    format!("MSVC_SPECTRE_LIB_DIR_{}", target.replace('-', "_"))
}

/// Builds the `lib\spectre\<arch>` directory beneath an MSVC toolchain root.
///
/// `base` is typically the value of the `VCToolsInstallDir` environment
/// variable, which is the MSVC build-tools install directory. Requiring a
/// [`SpectreArch`] rather than a string keeps an architecture name that has no
/// mapping from reaching the path, and joining the components avoids
/// hardcoding a separator or climbing parent directories relative to `cl.exe`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// use msvc_spectre_libs::resolve::{SpectreArch, spectre_lib_dir};
///
/// let dir = spectre_lib_dir(Path::new("C:/VC/Tools/MSVC/14.40"), SpectreArch::X64);
/// assert!(dir.ends_with("lib/spectre/x64"));
/// ```
#[must_use]
pub fn spectre_lib_dir(base: &Path, arch: SpectreArch) -> PathBuf {
    base.join("lib").join("spectre").join(arch.dir_name())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::Path;

    use super::{SpectreArch, override_var_name, spectre_lib_dir};

    #[test]
    fn maps_known_architectures() {
        assert_eq!(SpectreArch::from_target_arch("x86_64"), Some(SpectreArch::X64));
        assert_eq!(SpectreArch::from_target_arch("x86"), Some(SpectreArch::X86));
        assert_eq!(SpectreArch::from_target_arch("aarch64"), Some(SpectreArch::Arm64));
        assert_eq!(SpectreArch::from_target_arch("arm64ec"), Some(SpectreArch::Arm64));
        assert_eq!(SpectreArch::from_target_arch("arm"), Some(SpectreArch::Arm));
    }

    #[test]
    fn returns_none_for_unknown_architecture() {
        assert_eq!(SpectreArch::from_target_arch("riscv64"), None);
        assert_eq!(SpectreArch::from_target_arch(""), None);
    }

    #[test]
    fn renders_the_toolchain_directory_name() {
        assert_eq!(SpectreArch::X64.dir_name(), "x64");
        assert_eq!(SpectreArch::X86.dir_name(), "x86");
        assert_eq!(SpectreArch::Arm64.dir_name(), "arm64");
        assert_eq!(SpectreArch::Arm.dir_name(), "arm");
        // `Display` renders the same directory component.
        assert_eq!(SpectreArch::Arm64.to_string(), "arm64");
    }

    #[test]
    fn builds_spectre_lib_dir_beneath_the_toolchain_root() {
        let dir = spectre_lib_dir(Path::new("C:/VC/Tools/MSVC/14.40"), SpectreArch::Arm64);
        assert!(dir.ends_with("lib/spectre/arm64"));
        assert!(dir.starts_with("C:/VC/Tools/MSVC/14.40"));
    }

    #[test]
    fn builds_target_specific_override_name() {
        assert_eq!(
            override_var_name("x86_64-pc-windows-msvc"),
            "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc"
        );
        assert_eq!(
            override_var_name("aarch64-pc-windows-msvc"),
            "MSVC_SPECTRE_LIB_DIR_aarch64_pc_windows_msvc"
        );
    }
}
