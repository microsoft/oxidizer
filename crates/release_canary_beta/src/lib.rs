// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Canary crate used to exercise the release pipeline.
//!
//! This crate exists solely as a low-risk publish target so that changes to the release
//! infrastructure can be validated end-to-end without touching production crates. It exposes
//! a single function that returns a constant identifier; the value is intentionally trivial
//! so that the crate's behavior never needs to change between releases.
//!
//! # Examples
//!
//! ```
//! assert_eq!(release_canary_beta::canary_name(), "beta");
//! ```

/// Returns the name of this canary crate.
///
/// The value is stable across releases and is intended to be used by release pipeline
/// smoke tests that verify a crate was packaged and published correctly.
#[must_use]
pub fn canary_name() -> &'static str {
    "beta"
}

#[cfg(test)]
mod tests {
    use crate::canary_name;

    #[test]
    fn name_is_stable() {
        assert_eq!(canary_name(), "beta");
    }
}
