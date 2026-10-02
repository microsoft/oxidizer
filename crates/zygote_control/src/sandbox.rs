// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-platform sandbox intent shared by platform backends.

use std::io;

/// The requested privilege relationship between the controller and child.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrivilegeIntent {
    /// Preserve the launch identity and inherited privilege state.
    #[default]
    Inherit,
    /// Require the platform backend to create a reduction-only identity.
    Reduce,
}

/// Portable sandbox requirements for one launch.
///
/// Platform-specific mechanisms are configured through the cfg-gated `linux`
/// and `windows` modules. A non-default portable requirement is never
/// ignored: unsupported targets return [`io::ErrorKind::Unsupported`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SandboxPolicy {
    privilege_intent: PrivilegeIntent,
}

impl SandboxPolicy {
    /// Creates an empty policy.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            privilege_intent: PrivilegeIntent::Inherit,
        }
    }

    /// Requires a reduction-only child identity.
    ///
    /// Platform-native settings may strengthen this guarantee but cannot
    /// weaken the platform's portable reduction policy.
    #[must_use]
    pub const fn reduce_privileges(mut self) -> Self {
        self.privilege_intent = PrivilegeIntent::Reduce;
        self
    }

    /// Returns the configured privilege intent.
    #[must_use]
    pub const fn privilege_intent(self) -> PrivilegeIntent {
        self.privilege_intent
    }

    pub(super) const fn is_empty(self) -> bool {
        matches!(self.privilege_intent, PrivilegeIntent::Inherit)
    }

    /// Checks whether this target family implements the portable policy.
    ///
    /// This check does not probe runtime kernel, token, or account
    /// capabilities. Platform-specific probes can provide more detail, and a
    /// launch can still fail when the host cannot apply the requested
    /// reduction.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when a requested guarantee has
    /// no implementation on the current target.
    #[expect(clippy::unnecessary_wraps, reason = "support is target-dependent")]
    pub fn probe(self) -> io::Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        #[cfg(any(target_os = "linux", windows))]
        {
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "privilege reduction is unsupported on this target",
            ))
        }
    }
}
