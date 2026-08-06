// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pure helpers for locating the Spectre-mitigated MSVC CRT libraries.
//!
//! These functions perform no I/O, so they can be unit tested and reused both
//! by this crate's build script and by build-system integrators that want to
//! compute the override variable names themselves.

/// Maps a Rust target architecture (the value of the `CARGO_CFG_TARGET_ARCH`
/// build-script environment variable) to the name of the MSVC toolchain's
/// `lib\spectre\<arch>` subdirectory.
///
/// Returns [`None`] for architectures that have no Spectre-mitigated CRT.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs::resolve::spectre_arch;
///
/// assert_eq!(spectre_arch("x86_64"), Some("x64"));
/// assert_eq!(spectre_arch("x86"), Some("x86"));
/// assert_eq!(spectre_arch("aarch64"), Some("arm64"));
/// // `arm64ec` objects ship inside the `arm64` Spectre libraries.
/// assert_eq!(spectre_arch("arm64ec"), Some("arm64"));
/// assert_eq!(spectre_arch("riscv64"), None);
/// ```
#[must_use]
pub fn spectre_arch(target_arch: &str) -> Option<&'static str> {
    Some(match target_arch {
        "x86_64" => "x64",
        "x86" => "x86",
        // The `arm64ec` Spectre directory ships no libraries of its own; the
        // `arm64` libraries contain both `arm64` and `arm64ec` objects.
        "aarch64" | "arm64ec" => "arm64",
        // 32-bit ARM libraries live in `lib\spectre\arm`, matching the `x86`,
        // `x64`, and `arm64` sibling directories in the MSVC toolchain layout.
        "arm" => "arm",
        _ => return None,
    })
}

/// Computes the target-specific override environment variable name for a Rust
/// target triple.
///
/// The name is formed by replacing every `-` in the triple with `_` and
/// prefixing `MSVC_SPECTRE_LIB_DIR_`. Setting this variable (or the
/// target-agnostic `MSVC_SPECTRE_LIB_DIR`) to an existing directory makes the
/// build script use that directory verbatim instead of probing the toolchain.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs::resolve::override_var_name;
///
/// assert_eq!(override_var_name("x86_64-pc-windows-msvc"), "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc");
/// ```
#[must_use]
pub fn override_var_name(target: &str) -> String {
    format!("MSVC_SPECTRE_LIB_DIR_{}", target.replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::{override_var_name, spectre_arch};

    #[test]
    fn maps_known_architectures() {
        assert_eq!(spectre_arch("x86_64"), Some("x64"));
        assert_eq!(spectre_arch("x86"), Some("x86"));
        assert_eq!(spectre_arch("aarch64"), Some("arm64"));
        assert_eq!(spectre_arch("arm64ec"), Some("arm64"));
        assert_eq!(spectre_arch("arm"), Some("arm"));
    }

    #[test]
    fn returns_none_for_unknown_architecture() {
        assert_eq!(spectre_arch("riscv64"), None);
        assert_eq!(spectre_arch(""), None);
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
