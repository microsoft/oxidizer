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
/// use arty::runtime::{MonitorPolicy, Runtime};
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
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, reason = "Mutation tests cannot exercise this compile-time condition outside a test build.")]
    cfg!(any(test, doctest, miri)) {
        return true;
    }

    let Ok(executable) = std::env::current_exe() else {
        // #[gamma::skip(literal.bool_flip, reason = "Mutation tests cannot make current_exe fail without changing the environment probe.")]
        return false;
    };
    // #[gamma::skip(bool_expr.negate, call.replace_with_default, call_result.default, reason = "Mutation tests always use a test executable; the pure path classifier is tested separately.")]
    is_cargo_test_executable(&executable)
}

#[cfg(any(test, feature = "monitor"))]
fn is_cargo_test_executable(executable: &std::path::Path) -> bool {
    // cfg(test) does not reach a dependency used by an integration test.
    // Cargo places those test executables in the target profile's deps directory.
    if executable
        .parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|directory| directory == "deps")
    {
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
    fn cargo_test_executables_are_test_environments() {
        let integration_test = std::path::Path::new("target")
            .join("debug")
            .join("deps")
            .join("runtime_contracts-0123456789abcdef");
        let doctest = std::path::Path::new("target")
            .join("debug")
            .join("doctest")
            .join("rustdoctest012345")
            .join("rust_out");

        assert!(is_cargo_test_executable(&integration_test));
        assert!(is_cargo_test_executable(&doctest));
        assert!(!is_cargo_test_executable(
            std::path::Path::new("target").join("debug").join("application").as_path()
        ));
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
