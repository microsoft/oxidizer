// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{Error, ErrorKind, Result};
use std::path::{Component, Path, PathBuf};

/// Joins a relative `path` onto `base`, rejecting any traversal that would
/// escape the directory cone rooted at `base`.
///
/// Returns the fully resolved path on success.
///
/// # Limitations
///
/// This function performs purely lexical validation and does **not** resolve
/// symbolic links. A path such as `symlink_to_parent/../../etc/passwd` will
/// pass validation if the symlink component is treated as a normal directory
/// name.
pub fn safe_join(base: impl AsRef<Path>, relative: impl AsRef<Path>) -> Result<PathBuf> {
    let base = base.as_ref();
    let relative = relative.as_ref();

    let mut result = PathBuf::with_capacity(base.as_os_str().len() + 1 + relative.as_os_str().len());
    result.push(base);
    let mut depth: usize = 0;

    for component in relative.components() {
        match component {
            Component::Normal(c) => {
                #[cfg(windows)]
                if is_windows_reserved_name(c) {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "Windows reserved device names are not permitted",
                    ));
                }
                result.push(c);
                depth += 1;
            }
            Component::CurDir => {} // "." — skip
            Component::ParentDir => {
                if depth == 0 {
                    return Err(Error::new(ErrorKind::InvalidInput, "path escapes the directory"));
                }
                let _ = result.pop();
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "absolute paths are not permitted in capability-based access",
                ));
            }
        }
    }

    Ok(result)
}

/// Returns `true` if the given name (ignoring extension and case) is a
/// Windows reserved device name such as CON, PRN, NUL, etc.
#[cfg(windows)]
fn is_windows_reserved_name(name: &std::ffi::OsStr) -> bool {
    let s = name.to_ascii_uppercase();
    let s = s.to_string_lossy();
    // Strip any extension (e.g. "CON.txt" is still reserved)
    let stem = s.split('.').next().unwrap_or("");
    matches!(
        stem,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM0"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT0"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_relative() {
        let base = Path::new("/data");
        assert_eq!(
            safe_join(base, Path::new("foo/bar.txt")).expect("ok"),
            PathBuf::from("/data/foo/bar.txt")
        );
    }

    #[test]
    fn dot_segments() {
        let base = Path::new("/data");
        assert_eq!(
            safe_join(base, Path::new("./foo/./bar.txt")).expect("ok"),
            PathBuf::from("/data/foo/bar.txt")
        );
    }

    #[test]
    fn safe_dotdot() {
        let base = Path::new("/data");
        assert_eq!(
            safe_join(base, Path::new("foo/../bar.txt")).expect("ok"),
            PathBuf::from("/data/bar.txt")
        );
    }

    #[test]
    fn escape_rejected() {
        let base = Path::new("/data");
        let _ = safe_join(base, Path::new("../etc/passwd")).expect_err("should reject escape");
    }

    #[test]
    fn deep_escape_rejected() {
        let base = Path::new("/data");
        let _ = safe_join(base, Path::new("foo/../../etc/passwd")).expect_err("should reject deep escape");
    }

    #[test]
    fn absolute_rejected() {
        let base = Path::new("/data");
        let _ = safe_join(base, Path::new("/etc/passwd")).expect_err("should reject absolute path");
    }

    #[test]
    fn empty_path() {
        let base = Path::new("/data");
        assert_eq!(safe_join(base, Path::new("")).expect("ok"), PathBuf::from("/data"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_reserved_name_rejected() {
        let base = Path::new("C:\\data");
        let _ = safe_join(base, Path::new("CON")).expect_err("should reject CON");
        let _ = safe_join(base, Path::new("nul")).expect_err("should reject NUL");
        let _ = safe_join(base, Path::new("COM1.txt")).expect_err("should reject COM1.txt");
    }

    #[cfg(windows)]
    #[test]
    fn windows_non_reserved_name_allowed() {
        let base = Path::new("C:\\data");
        safe_join(base, Path::new("CONSOLE")).unwrap();
        safe_join(base, Path::new("connect.txt")).unwrap();
    }
}
