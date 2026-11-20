// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects the runtime's monitoring configuration.
///
/// Pass a policy to
/// [`RuntimeBuilder::monitor`](crate::runtime::RuntimeBuilder::monitor).
/// The automatic policy is the default.
///
/// The `monitor` Cargo feature enables the Seismograph dependencies that
/// monitoring needs. Building an enabled runtime starts a local monitor.
/// Task collection and poll timing run only when requested. This policy does
/// not enable event recording or install an allocator.
///
/// The automatic policy disables monitoring for tests, doctests, documentation
/// examples, and Miri. It enables monitoring in other environments. The exact
/// environments selected by the automatic policy can change in future releases.
///
/// # Examples
///
/// ```no_run
/// use arty::runtime::Runtime;
/// use arty::runtime::policy::MonitorPolicy;
///
/// let runtime = Runtime::builder()
///     .monitor(MonitorPolicy::enabled())
///     .build()?;
/// runtime.stop()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct MonitorPolicy {
    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(dead_code, reason = "The policy is only read when monitoring is compiled in.")
    )]
    selection: Selection,
}

#[derive(Debug, Clone)]
enum Selection {
    Auto,
    Enabled,
    Disabled,
}

impl MonitorPolicy {
    /// Creates a policy that selects monitoring for the current environment.
    ///
    /// Monitoring is disabled for tests, doctests, documentation examples, and
    /// Miri. It is enabled in other environments. This selection can change in
    /// future releases.
    #[must_use]
    pub const fn auto() -> Self {
        Self {
            selection: Selection::Auto,
        }
    }

    /// Creates a policy with monitoring enabled in the runtime configuration.
    #[must_use]
    pub const fn enabled() -> Self {
        Self {
            selection: Selection::Enabled,
        }
    }

    /// Creates a policy with monitoring disabled in the runtime configuration.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            selection: Selection::Disabled,
        }
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn is_enabled(&self) -> bool {
        self.is_enabled_for(is_test_environment())
    }

    #[cfg(any(test, feature = "monitor"))]
    const fn is_enabled_for(&self, test_environment: bool) -> bool {
        match &self.selection {
            Selection::Auto => !test_environment,
            Selection::Enabled => true,
            Selection::Disabled => false,
        }
    }
}

impl Default for MonitorPolicy {
    fn default() -> Self {
        Self::auto()
    }
}

#[cfg(any(test, feature = "monitor"))]
// #[gamma::skip(fn_value.bool_true, reason = "Mutation tests always run in a test executable, where this function must return true.")]
fn is_test_environment() -> bool {
    // #[gamma::skip(call_result.default, reason = "Mutation tests always run in a test executable, where the injected compile-time value is true.")]
    // #[gamma::skip(bool_expr.negate, reason = "Mutation tests compile this expression as true and cannot observe the opposite build configuration.")]
    is_test_environment_with(cfg!(any(test, doctest, miri)), std::env::current_exe)
}

#[cfg(any(test, feature = "monitor"))]
fn is_test_environment_with(
    compile_time_test_environment: bool,
    current_executable: impl FnOnce() -> std::io::Result<std::path::PathBuf>,
) -> bool {
    if compile_time_test_environment {
        return true;
    }

    let Ok(executable) = current_executable() else {
        return false;
    };
    is_cargo_test_executable(&executable)
}

#[cfg(any(test, feature = "monitor"))]
fn is_cargo_test_executable(executable: &std::path::Path) -> bool {
    // cfg(test) does not reach a dependency used by an integration test.
    // Cargo places those test executables in the target profile's deps directory
    // and appends a 16-digit hexadecimal crate hash to the target name.
    let cargo_test_binary = executable
        .parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|directory| directory == "deps")
        && executable.file_stem().and_then(std::ffi::OsStr::to_str).is_some_and(|stem| {
            stem.rsplit_once('-')
                .is_some_and(|(_, hash)| hash.len() == 16 && hash.as_bytes().iter().all(u8::is_ascii_hexdigit))
        });
    if cargo_test_binary {
        return true;
    }

    // Rustdoc runs documentation examples from a rustdoctest-prefixed temporary directory.
    executable.ancestors().any(|directory| {
        directory
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("rustdoctest"))
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
// Monitoring currently stores configuration without a public observable effect.
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_automatic() {
        assert!(matches!(MonitorPolicy::default().selection, Selection::Auto));
    }

    #[test]
    fn automatic_policy_disables_monitoring_in_tests() {
        assert!(!MonitorPolicy::auto().is_enabled());
        assert!(!MonitorPolicy::auto().is_enabled_for(true));
    }

    #[test]
    fn automatic_policy_enables_monitoring_outside_tests() {
        assert!(MonitorPolicy::auto().is_enabled_for(false));
    }

    #[test]
    fn environment_detection_uses_compile_time_test_context() {
        assert!(is_test_environment_with(true, || {
            Err(std::io::Error::other("the executable probe must not run"))
        }));
    }

    #[test]
    fn environment_detection_treats_an_unavailable_executable_as_not_a_test() {
        assert!(!is_test_environment_with(false, || {
            Err(std::io::Error::other("current executable unavailable"))
        }));
    }

    #[test]
    fn cargo_test_executables_are_test_environments() {
        let integration_test = std::path::Path::new("target")
            .join("debug")
            .join("deps")
            .join("runtime_contracts-0123456789abcdef");
        let deployed_application = std::path::Path::new("opt").join("service").join("deps").join("service");
        let hashed_deployed_application = std::path::Path::new("opt")
            .join("service")
            .join("runtime_contracts-0123456789abcdef");
        let unhashed_cargo_artifact = std::path::Path::new("target").join("debug").join("deps").join("runtime_contracts");
        let short_hash = std::path::Path::new("target")
            .join("debug")
            .join("deps")
            .join("runtime_contracts-0123456789abcde");
        let nonhex_hash = std::path::Path::new("target")
            .join("debug")
            .join("deps")
            .join("runtime_contracts-g123456789abcdef");
        let doctest = std::path::Path::new("target")
            .join("debug")
            .join("doctest")
            .join("rustdoctest012345")
            .join("rust_out");
        let application = std::path::Path::new("target").join("debug").join("application");

        assert!(is_test_environment_with(false, || Ok(integration_test)));
        assert!(is_test_environment_with(false, || Ok(doctest)));
        assert!(!is_test_environment_with(false, || Ok(deployed_application)));
        assert!(!is_test_environment_with(false, || Ok(hashed_deployed_application)));
        assert!(!is_test_environment_with(false, || Ok(unhashed_cargo_artifact)));
        assert!(!is_test_environment_with(false, || Ok(short_hash)));
        assert!(!is_test_environment_with(false, || Ok(nonhex_hash)));
        assert!(!is_test_environment_with(false, || Ok(application)));
    }

    #[test]
    fn enabled_policy_enables_monitoring_in_tests() {
        assert!(MonitorPolicy::enabled().is_enabled_for(true));
    }

    #[test]
    fn disabled_policy_disables_monitoring() {
        assert!(!MonitorPolicy::disabled().is_enabled());
    }
}
