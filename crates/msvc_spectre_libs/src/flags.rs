// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pure helpers for reasoning about the flags that reached `rustc`.
//!
//! The build script uses these to stay *idempotent* (never add a link-search
//! path that is already in effect) and to *verify* that the constant security
//! flags an integrator requires actually reached `rustc`. Both concerns are
//! expressed as pure functions over already-captured environment strings so
//! they can be unit tested without touching the environment.
//!
//! # Why verification is needed
//!
//! Cargo has four mutually exclusive sources of extra `rustc` flags. A
//! `RUSTFLAGS` environment variable **replaces** — it does not merge with —
//! `target.<triple>.rustflags` from `.cargo/config.toml`. So an unrelated
//! ambient `RUSTFLAGS` silently drops every security flag configured in
//! `.cargo/config.toml`, producing a binary that quietly fails its security
//! requirements. Checking [`CARGO_ENCODED_RUSTFLAGS_VAR`] from a build script
//! turns that silent drop into a build diagnostic.

use std::path::Path;

/// Environment variable through which Cargo reports the final `rustc` flags to
/// build scripts. Entries are separated by the ASCII unit separator (`0x1f`).
pub const CARGO_ENCODED_RUSTFLAGS_VAR: &str = "CARGO_ENCODED_RUSTFLAGS";

/// Separator Cargo uses inside [`CARGO_ENCODED_RUSTFLAGS_VAR`].
const ENCODED_SEPARATOR: char = '\x1f';

/// Environment variable listing the linker arguments that must reach every
/// final Windows MSVC artifact, separated by `;`.
///
/// Empty or unset — the default — disables the check entirely, so the crate
/// imposes no policy of its own. Which arguments are required depends on the
/// toolchain in use and on the compliance bar that integrators must meet, so
/// only the integrator can supply the list.
///
/// Set it in `.cargo/config.toml` under `[env]` rather than in the ambient
/// environment, so that every entry point into the build agrees on the value:
///
/// ```toml
/// [env]
/// MSVC_SPECTRE_REQUIRED_LINK_ARGS = "/CETCOMPAT"
/// ```
///
/// Listing an argument the toolchain already emits (commonly `/guard:cf`,
/// `/DYNAMICBASE`, `/HIGHENTROPYVA`, and `/NXCOMPAT`) would demand a redundant
/// second copy of a flag that is already in effect; verify with
/// `dumpbin /headers` before adding one.
pub const REQUIRED_LINK_ARGS_VAR: &str = "MSVC_SPECTRE_REQUIRED_LINK_ARGS";

/// Splits the `;`-separated value of [`REQUIRED_LINK_ARGS_VAR`] into
/// individual linker arguments, ignoring empty and whitespace-only entries.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs::flags::required_link_args;
///
/// assert_eq!(required_link_args(" /CETCOMPAT ; ;/guard:ehcont"), vec!["/CETCOMPAT", "/guard:ehcont"]);
/// assert!(required_link_args("").is_empty());
/// ```
#[must_use]
pub fn required_link_args(value: &str) -> Vec<&str> {
    value.split(';').map(str::trim).filter(|arg| !arg.is_empty()).collect()
}

/// Normalizes the encoded `rustc` flag list into individual codegen values.
///
/// Cargo may deliver a codegen option either as one argument
/// (`-Clink-arg=/CETCOMPAT`) or as two (`-C`, `link-arg=/CETCOMPAT`); both
/// spellings are folded to the bare value (`link-arg=/CETCOMPAT`). Arguments
/// that are not codegen options are dropped.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs::flags::codegen_values;
///
/// let encoded = "-C\u{1f}link-arg=/CETCOMPAT\u{1f}-Ctarget-cpu=x86-64-v3";
/// assert_eq!(
///     codegen_values(encoded),
///     vec!["link-arg=/CETCOMPAT".to_owned(), "target-cpu=x86-64-v3".to_owned()]
/// );
/// ```
#[must_use]
pub fn codegen_values(encoded: &str) -> Vec<String> {
    let args: Vec<&str> = encoded.split(ENCODED_SEPARATOR).filter(|arg| !arg.is_empty()).collect();

    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if arg == "-C" {
            // Split spelling: the value is the next argument. A trailing `-C`
            // with no value is malformed; ignore it rather than panicking.
            if let Some(value) = args.get(index + 1) {
                values.push((*value).to_owned());
            }
            index += 2;
        } else {
            if let Some(value) = arg.strip_prefix("-C") {
                values.push(value.to_owned());
            }
            index += 1;
        }
    }
    values
}

/// Returns the entries of `required` that are **not** present in `encoded`.
///
/// Linker option names are case-insensitive, so comparison ignores case.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs::flags::missing_required_link_args;
///
/// assert!(missing_required_link_args("-Clink-arg=/cetcompat", &["/CETCOMPAT"]).is_empty());
/// assert_eq!(missing_required_link_args("", &["/CETCOMPAT"]), vec!["/CETCOMPAT"]);
/// ```
#[must_use]
pub fn missing_required_link_args<'a>(encoded: &str, required: &[&'a str]) -> Vec<&'a str> {
    let values = codegen_values(encoded);
    required
        .iter()
        .filter(|required| {
            !values.iter().any(|value| {
                value
                    .split_once('=')
                    .is_some_and(|(key, argument)| key.eq_ignore_ascii_case("link-arg") && argument.eq_ignore_ascii_case(required))
            })
        })
        .copied()
        .collect()
}

/// Returns whether `encoded` already adds `dir` as a native library search
/// path (`-L native=<dir>` or the bare `-L <dir>` form).
///
/// Used to keep the build script idempotent when an integrator has already
/// supplied the Spectre directory through `RUSTFLAGS`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// use msvc_spectre_libs::flags::adds_link_search;
///
/// let encoded = "-L\u{1f}native=C:\\VC\\lib\\spectre\\x64";
/// assert!(adds_link_search(encoded, Path::new("C:/VC/lib/spectre/x64")));
/// assert!(!adds_link_search(encoded, Path::new("C:/VC/lib/spectre/arm64")));
/// ```
#[must_use]
pub fn adds_link_search(encoded: &str, dir: &Path) -> bool {
    let args: Vec<&str> = encoded.split(ENCODED_SEPARATOR).filter(|arg| !arg.is_empty()).collect();

    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        // `-L` takes its value either joined (`-Lnative=<dir>`) or split
        // (`-L`, `native=<dir>`).
        let value = if arg == "-L" {
            index += 2;
            args.get(index - 1).copied()
        } else {
            index += 1;
            arg.strip_prefix("-L")
        };

        // Strip the optional `<kind>=` prefix (`native=`, `dependency=`, ...).
        // A bare Windows path such as `C:\lib` also contains `=`-free text, so
        // only strip a prefix that is a known-shaped kind (no path separator).
        if let Some(value) = value {
            let path = match value.split_once('=') {
                Some((kind, rest)) if !kind.contains(['\\', '/', ':']) => rest,
                _ => value,
            };
            if same_path(Path::new(path), dir) {
                return true;
            }
        }
    }
    false
}

/// Returns whether the `LIB` environment variable (which `link.exe` reads
/// directly) already contains `dir`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// use msvc_spectre_libs::flags::lib_var_contains;
///
/// let lib = "C:\\sdk\\um\\x64;C:\\VC\\lib\\spectre\\x64;";
/// assert!(lib_var_contains(lib, Path::new("C:/VC/lib/spectre/x64")));
/// assert!(!lib_var_contains(lib, Path::new("C:/VC/lib/spectre/arm64")));
/// ```
#[must_use]
pub fn lib_var_contains(lib: &str, dir: &Path) -> bool {
    lib.split(';')
        .filter(|entry| !entry.trim().is_empty())
        .any(|entry| same_path(Path::new(entry.trim()), dir))
}

/// Compares two Windows paths for equality, ignoring case, separator style,
/// and a trailing separator.
///
/// Purely lexical: it performs no filesystem access, so it never resolves
/// symlinks or junctions. That is sufficient here, because every path being
/// compared is derived from the same toolchain root.
fn same_path(left: &Path, right: &Path) -> bool {
    fn normalize(path: &Path) -> String {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }
    normalize(left) == normalize(right)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{adds_link_search, codegen_values, lib_var_contains, missing_required_link_args, required_link_args, same_path};

    #[test]
    fn splits_required_link_args_and_ignores_blank_entries() {
        assert_eq!(
            required_link_args(" /CETCOMPAT ; ;/guard:ehcont"),
            vec!["/CETCOMPAT", "/guard:ehcont"]
        );
        assert!(required_link_args("").is_empty());
        assert!(required_link_args("  ;; ").is_empty());
    }

    #[test]
    fn decodes_joined_and_split_codegen_options() {
        let encoded = "-C\u{1f}link-arg=/CETCOMPAT\u{1f}-Ctarget-cpu=x86-64-v3";
        assert_eq!(
            codegen_values(encoded),
            vec!["link-arg=/CETCOMPAT".to_owned(), "target-cpu=x86-64-v3".to_owned()]
        );
    }

    #[test]
    fn ignores_non_codegen_and_malformed_arguments() {
        // A trailing bare `-C` has no value and must not panic.
        let encoded = "-L\u{1f}native=C:\\lib\u{1f}--cfg\u{1f}foo\u{1f}-C";
        assert!(codegen_values(encoded).is_empty());
        assert!(codegen_values("").is_empty());
    }

    #[test]
    fn reports_no_missing_link_args_when_all_present() {
        assert!(missing_required_link_args("-Clink-arg=/CETCOMPAT", &["/CETCOMPAT"]).is_empty());
        // Linker options are case-insensitive.
        assert!(missing_required_link_args("-C\u{1f}link-arg=/cetcompat", &["/CETCOMPAT"]).is_empty());
        // An empty requirement list can never be violated.
        assert!(missing_required_link_args("", &[]).is_empty());
    }

    #[test]
    fn reports_missing_link_args() {
        assert_eq!(missing_required_link_args("", &["/CETCOMPAT"]), vec!["/CETCOMPAT"]);
        // A different link-arg does not satisfy the requirement.
        assert_eq!(
            missing_required_link_args("-Clink-arg=/DYNAMICBASE", &["/CETCOMPAT"]),
            vec!["/CETCOMPAT"]
        );
        // `target-cpu` is a codegen option but not a link-arg.
        assert_eq!(
            missing_required_link_args("-Ctarget-cpu=x86-64-v3", &["/CETCOMPAT"]),
            vec!["/CETCOMPAT"]
        );
        // Only the unsatisfied entries are reported.
        assert_eq!(
            missing_required_link_args("-Clink-arg=/CETCOMPAT", &["/CETCOMPAT", "/guard:ehcont"]),
            vec!["/guard:ehcont"]
        );
    }

    #[test]
    fn detects_existing_link_search_in_both_spellings() {
        let dir = Path::new("C:/VC/lib/spectre/x64");
        assert!(adds_link_search("-L\u{1f}native=C:\\VC\\lib\\spectre\\x64", dir));
        assert!(adds_link_search("-Lnative=C:\\VC\\lib\\spectre\\x64", dir));
        // Bare path with no `<kind>=` prefix.
        assert!(adds_link_search("-L\u{1f}C:\\VC\\lib\\spectre\\x64", dir));
        // A drive-letter path must not be mistaken for a `<kind>=` prefix.
        assert!(adds_link_search("-L\u{1f}C:\\VC\\lib\\spectre\\x64\\", dir));
    }

    #[test]
    fn does_not_detect_unrelated_link_search() {
        let dir = Path::new("C:/VC/lib/spectre/x64");
        assert!(!adds_link_search("", dir));
        assert!(!adds_link_search("-L\u{1f}native=C:\\VC\\lib\\spectre\\arm64", dir));
        // `dependency=` paths are search paths too, but for a different dir.
        assert!(!adds_link_search("-L\u{1f}dependency=C:\\target\\debug\\deps", dir));
        // A `-C` option that merely mentions the path is not a search path.
        assert!(!adds_link_search("-Clink-arg=C:\\VC\\lib\\spectre\\x64", dir));
    }

    #[test]
    fn detects_directory_in_lib_variable() {
        let lib = "C:\\sdk\\um\\x64;C:\\VC\\lib\\spectre\\x64;";
        assert!(lib_var_contains(lib, Path::new("C:/VC/lib/spectre/x64")));
        assert!(!lib_var_contains(lib, Path::new("C:/VC/lib/spectre/arm64")));
        assert!(!lib_var_contains("", Path::new("C:/VC/lib/spectre/x64")));
        // Empty and whitespace-only entries are ignored.
        assert!(!lib_var_contains(";; ;", Path::new("C:/VC/lib/spectre/x64")));
    }

    #[test]
    fn compares_paths_ignoring_case_separators_and_trailing_slash() {
        assert!(same_path(Path::new("C:\\VC\\Lib"), Path::new("c:/vc/lib")));
        assert!(same_path(Path::new("C:\\VC\\Lib\\"), Path::new("C:\\VC\\Lib")));
        assert!(!same_path(Path::new("C:\\VC\\Lib"), Path::new("C:\\VC\\Lib2")));
    }
}
