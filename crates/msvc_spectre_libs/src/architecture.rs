// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::{Path, PathBuf};

pub(crate) fn is_windows_msvc_target(target_os: &str, target_env: &str) -> bool {
    target_os == "windows" && target_env == "msvc"
}

pub(crate) const fn spectre_directory(target_arch: &str) -> Option<&'static str> {
    match target_arch.as_bytes() {
        b"x86_64" => Some("x64"),
        b"x86" => Some("x86"),
        // The spectre\arm64ec directory is empty. The arm64 libraries contain
        // both arm64 and arm64ec objects.
        b"aarch64" | b"arm64ec" => Some("arm64"),
        b"arm" => Some("arm32"),
        _ => None,
    }
}

pub(crate) fn spectre_libs_path(compiler_path: &Path, arch: &str) -> Option<PathBuf> {
    let version_directory = compiler_path.parent()?.parent()?.parent()?.parent()?;
    Some(version_directory.join("lib").join("spectre").join(arch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_windows_msvc_targets() {
        assert!(is_windows_msvc_target("windows", "msvc"));
        assert!(!is_windows_msvc_target("windows", "gnu"));
        assert!(!is_windows_msvc_target("linux", "msvc"));
    }

    #[test]
    fn maps_supported_architectures() {
        let expected = [
            ("x86_64", "x64"),
            ("x86", "x86"),
            ("aarch64", "arm64"),
            ("arm64ec", "arm64"),
            ("arm", "arm32"),
        ];

        for (target_arch, directory) in expected {
            assert_eq!(spectre_directory(target_arch), Some(directory));
        }
    }

    #[test]
    fn rejects_unsupported_architectures() {
        assert_eq!(spectre_directory("mips"), None);
    }

    #[test]
    fn finds_spectre_libraries_relative_to_compiler() {
        let version_directory = Path::new("VC").join("Tools").join("MSVC").join("14.44.35207");
        let compiler = version_directory.join("bin").join("Hostx64").join("x64").join("cl.exe");

        assert_eq!(
            spectre_libs_path(&compiler, "x64"),
            Some(version_directory.join("lib").join("spectre").join("x64"))
        );
    }

    #[test]
    fn rejects_compiler_paths_outside_toolchain_layout() {
        assert_eq!(spectre_libs_path(Path::new("cl.exe"), "x64"), None);
    }
}
