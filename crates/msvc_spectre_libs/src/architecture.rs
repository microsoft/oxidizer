// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
