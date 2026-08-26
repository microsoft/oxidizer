// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pure helpers for reasoning about the flags that reached `rustc`.
//!
//! The build script uses these to verify that the required linker arguments an
//! integrator configured actually reached `rustc`. Every helper is a pure
//! function over an already-captured environment string, so it can be unit
//! tested without touching the process environment.
//!
//! # Why verification is needed
//!
//! Cargo has several mutually exclusive sources of extra `rustc` flags. A
//! `RUSTFLAGS` environment variable **replaces** — it does not merge with —
//! `target.<triple>.rustflags` from `.cargo/config.toml`. So an unrelated
//! ambient `RUSTFLAGS` silently drops every required linker argument
//! configured in `.cargo/config.toml`, producing an artifact that quietly
//! fails its compliance requirements. Checking
//! [`CARGO_ENCODED_RUSTFLAGS_VAR`] from a build script turns that silent drop
//! into a build diagnostic.
//!
//! # Assurance boundary
//!
//! These helpers inspect *configuration*: the flags Cargo reports it will pass
//! to `rustc`. They do not observe the linker invocation and do not inspect
//! the produced artifact, so a successful check establishes that the
//! configured arguments were requested, not that the final binary carries the
//! corresponding properties. Post-link inspection, for example
//! `dumpbin /headers`, remains the only artifact-level evidence.

/// Environment variable through which Cargo reports the final `rustc` flags.
///
/// Present for a build script run by any Cargo that reports the flags at all;
/// a version that does not set it leaves nothing to verify, which the policy
/// treats as "no requirement to check" rather than as a failure. Entries are
/// separated by the ASCII unit separator control character, which cannot occur
/// inside a flag, so the encoding is unambiguous; see [`codegen_values`] for
/// the accepted spellings.
pub const CARGO_ENCODED_RUSTFLAGS_VAR: &str = "CARGO_ENCODED_RUSTFLAGS";

/// Separator Cargo uses inside [`CARGO_ENCODED_RUSTFLAGS_VAR`].
const ENCODED_SEPARATOR: char = '\x1f';

/// Environment variable listing the required linker arguments.
///
/// The value is a `;`-separated list of arguments that must reach every final
/// Windows MSVC artifact. Empty or unset — the default — disables the check
/// entirely, so the crate imposes no policy of its own. Which arguments are
/// required depends on the toolchain in use and on the compliance requirements
/// that integrators must meet, so only the integrator can supply the list.
///
/// # Target scope
///
/// The `[env]` table of Cargo is visible for every selected target, but a
/// linker argument is often architecture-specific: `/CETCOMPAT`, for example,
/// is documented for `x64` only. Prefer the target-suffixed spelling returned
/// by [`required_link_args_var_name`], which takes precedence over this
/// target-agnostic name; use this one only for a requirement that genuinely
/// applies to every Windows MSVC target.
///
/// # Encoding
///
/// Entries are separated by `;` with no escape mechanism, so an argument that
/// itself contains a semicolon cannot be represented. That is deliberate: the
/// contract covers switch-style hardening arguments such as `/CETCOMPAT` and
/// `/guard:ehcont`, none of which contain a semicolon. An encoding with
/// defined escaping would have to be introduced before the contract could be
/// widened to arbitrary linker arguments.
///
/// # Usage
///
/// Set it in `.cargo/config.toml` under `[env]` rather than in the ambient
/// environment, so that every entry point into the build agrees on the value:
///
/// ```toml
/// [env]
/// MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc = "/CETCOMPAT"
/// ```
///
/// Listing an argument the toolchain already emits (commonly `/guard:cf`,
/// `/DYNAMICBASE`, `/HIGHENTROPYVA`, and `/NXCOMPAT`) would demand a redundant
/// second copy of a flag that is already in effect; verify with
/// `dumpbin /headers` before adding one.
pub const REQUIRED_LINK_ARGS_VAR: &str = "MSVC_SPECTRE_REQUIRED_LINK_ARGS";

/// Computes the target-specific name of [`REQUIRED_LINK_ARGS_VAR`].
///
/// The name is formed by appending the target triple, with every `-` replaced
/// by `_`. The build script prefers this variable over the target-agnostic
/// name, so a requirement that applies to one architecture does not produce a
/// false diagnostic on another.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs_build::flags::required_link_args_var_name;
///
/// assert_eq!(
///     required_link_args_var_name("x86_64-pc-windows-msvc"),
///     "MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc"
/// );
/// ```
#[must_use]
pub fn required_link_args_var_name(target: &str) -> String {
    format!("{REQUIRED_LINK_ARGS_VAR}_{}", target.replace('-', "_"))
}

/// Splits a required-linker-argument list into individual arguments.
///
/// The value is the `;`-separated contents of [`REQUIRED_LINK_ARGS_VAR`].
/// Empty and whitespace-only entries are ignored.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs_build::flags::required_link_args;
///
/// assert_eq!(
///     required_link_args(" /CETCOMPAT ; ;/guard:ehcont"),
///     vec!["/CETCOMPAT", "/guard:ehcont"]
/// );
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
/// use msvc_spectre_libs_build::flags::codegen_values;
///
/// let encoded = "-C\u{1f}link-arg=/CETCOMPAT\u{1f}-Ctarget-cpu=x86-64-v3";
/// assert_eq!(
///     codegen_values(encoded),
///     vec![
///         "link-arg=/CETCOMPAT".to_owned(),
///         "target-cpu=x86-64-v3".to_owned()
///     ]
/// );
/// ```
#[must_use]
pub fn codegen_values(encoded: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut args = encoded.split(ENCODED_SEPARATOR).filter(|arg| !arg.is_empty());

    while let Some(arg) = args.next() {
        if arg == "-C" {
            // Split spelling: the value is the next argument. A trailing `-C`
            // with no value is malformed; ignore it rather than panicking.
            if let Some(value) = args.next() {
                values.push(value.to_owned());
            }
        } else if let Some(value) = arg.strip_prefix("-C") {
            values.push(value.to_owned());
        }
    }
    values
}

/// Returns the entries of `required` that did not reach `rustc`.
///
/// `encoded` is the value of [`CARGO_ENCODED_RUSTFLAGS_VAR`]. Both codegen
/// options that append linker arguments are recognized: `link-arg`, which
/// carries exactly one argument, and `link-args`, which carries several
/// separated by whitespace. Linker option names are case-insensitive, so
/// comparison ignores case.
///
/// # Examples
///
/// ```
/// use msvc_spectre_libs_build::flags::missing_required_link_args;
///
/// assert!(missing_required_link_args("-Clink-arg=/cetcompat", &["/CETCOMPAT"]).is_empty());
/// // `link-args` carries several whitespace-separated arguments.
/// assert!(
///     missing_required_link_args("-Clink-args=/CETCOMPAT /guard:ehcont", &["/guard:ehcont"])
///         .is_empty()
/// );
/// assert_eq!(
///     missing_required_link_args("", &["/CETCOMPAT"]),
///     vec!["/CETCOMPAT"]
/// );
/// ```
#[must_use]
pub fn missing_required_link_args<'a>(encoded: &str, required: &[&'a str]) -> Vec<&'a str> {
    let values = codegen_values(encoded);
    required
        .iter()
        .filter(|required| !values.iter().any(|value| supplies_link_arg(value, required)))
        .copied()
        .collect()
}

/// Returns whether one codegen value appends `required` to the linker command.
///
/// `value` is a codegen option with its `-C` prefix already stripped, as
/// produced by [`codegen_values`].
fn supplies_link_arg(value: &str, required: &str) -> bool {
    let Some((key, arguments)) = value.split_once('=') else {
        return false;
    };

    // `link-arg` takes the whole value as a single argument, so it is compared
    // as-is; `link-args` takes several separated by whitespace, matching how
    // rustc forwards them to the linker.
    if key.eq_ignore_ascii_case("link-arg") {
        arguments.eq_ignore_ascii_case(required)
    } else if key.eq_ignore_ascii_case("link-args") {
        arguments.split_whitespace().any(|argument| argument.eq_ignore_ascii_case(required))
    } else {
        false
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{codegen_values, missing_required_link_args, required_link_args, required_link_args_var_name};

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
    fn builds_target_specific_required_link_args_name() {
        assert_eq!(
            required_link_args_var_name("x86_64-pc-windows-msvc"),
            "MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc"
        );
        assert_eq!(
            required_link_args_var_name("aarch64-pc-windows-msvc"),
            "MSVC_SPECTRE_REQUIRED_LINK_ARGS_aarch64_pc_windows_msvc"
        );
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
    fn accepts_requirements_supplied_through_link_args() {
        // `link-args` carries several whitespace-separated linker arguments,
        // in both the joined and the split `-C` spelling.
        let required = ["/CETCOMPAT", "/guard:ehcont"];
        assert!(missing_required_link_args("-Clink-args=/CETCOMPAT /guard:ehcont", &required).is_empty());
        assert!(missing_required_link_args("-C\u{1f}link-args=/cetcompat  /GUARD:EHCONT", &required).is_empty());
        // A single-valued `link-args` is still a list of one.
        assert!(missing_required_link_args("-Clink-args=/CETCOMPAT", &["/CETCOMPAT"]).is_empty());
        // Only the arguments actually listed are satisfied.
        assert_eq!(
            missing_required_link_args("-Clink-args=/CETCOMPAT", &required),
            vec!["/guard:ehcont"]
        );
    }

    #[test]
    fn reports_missing_link_args() {
        assert_eq!(missing_required_link_args("", &["/CETCOMPAT"]), vec!["/CETCOMPAT"]);
        // A different linker argument does not satisfy the requirement.
        assert_eq!(
            missing_required_link_args("-Clink-arg=/DYNAMICBASE", &["/CETCOMPAT"]),
            vec!["/CETCOMPAT"]
        );
        // `target-cpu` is a codegen option but does not append linker arguments.
        assert_eq!(
            missing_required_link_args("-Ctarget-cpu=x86-64-v3", &["/CETCOMPAT"]),
            vec!["/CETCOMPAT"]
        );
        // A codegen option without a value cannot supply anything.
        assert_eq!(missing_required_link_args("-Cdebuginfo", &["/CETCOMPAT"]), vec!["/CETCOMPAT"]);
        // `link-arg` takes exactly one argument, so a whitespace-separated
        // list supplied through it does not satisfy either entry.
        assert_eq!(
            missing_required_link_args("-Clink-arg=/CETCOMPAT /guard:ehcont", &["/CETCOMPAT"]),
            vec!["/CETCOMPAT"]
        );
        // Only the unsatisfied entries are reported.
        assert_eq!(
            missing_required_link_args("-Clink-arg=/CETCOMPAT", &["/CETCOMPAT", "/guard:ehcont"]),
            vec!["/guard:ehcont"]
        );
    }
}
