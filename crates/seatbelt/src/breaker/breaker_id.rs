// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Display;
use std::hash::Hasher;

/// Identifies one circuit breaker.
///
/// Each unique `BreakerId` has independent state.
/// The state includes failure counts, health metrics, and the circuit status.
/// Use separate IDs for separate backends or services.
///
/// Use a small, stable set of IDs.
/// Do not use a user ID or request ID.
/// A large set of IDs can cause unbounded memory use and can hide service-wide failures.
///
/// For lower cost, use integer IDs or static string IDs.
///
/// # Examples
///
/// ## Creation from a number
///
/// ```rust
/// use seatbelt::breaker::BreakerId;
///
/// let id = BreakerId::from(42_u64);
/// assert_eq!(id.to_string(), "42");
/// ```
///
/// ## Creation from HTTP request authority and scheme
///
/// ```rust
/// use seatbelt::breaker::BreakerId;
///
/// // Use the scheme and authority from an HTTP request.
/// let scheme = "https";
/// let authority = "api.example.com";
/// let id_value = format!("{}://{}", scheme, authority);
///
/// let id = BreakerId::from(id_value);
/// assert_eq!(id.to_string(), "https://api.example.com");
///
/// // A hashed ID needs a display label.
/// let hashed_id = BreakerId::hashed(&(scheme, authority), "scheme_and_authority");
/// assert_eq!(hashed_id.to_string(), "scheme_and_authority");
/// ```
///
/// # Telemetry
///
/// Logs and metrics can contain the breaker ID.
/// Do not put authentication tokens, personal data, or confidential data in an ID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakerId(BreakerIdValue);

impl BreakerId {
    pub(crate) const fn default() -> Self {
        Self(BreakerIdValue::String(Cow::Borrowed("default")))
    }

    /// Returns `true` if this ID is the default breaker ID.
    ///
    /// Used to fast-path the common single-breaker configuration, avoiding a lock,
    /// a hash, and a map probe when routing to the shared default engine.
    pub(crate) fn is_default(&self) -> bool {
        matches!(&self.0, BreakerIdValue::String(value) if value.as_ref() == "default")
    }

    /// Creates a breaker ID by hashing a value.
    ///
    /// The value must implement `Hash`.
    /// The `label` appears in display output and telemetry.
    pub fn hashed<T: std::hash::Hash>(value: &T, label: &'static str) -> Self {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        Self(BreakerIdValue::Hashed(hasher.finish(), label))
    }
}

impl Display for BreakerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            BreakerIdValue::Number(n) => write!(f, "{n}"),
            BreakerIdValue::String(s) => f.write_str(s),
            BreakerIdValue::Hashed(_, label) => f.write_str(label),
        }
    }
}

impl From<u64> for BreakerId {
    fn from(value: u64) -> Self {
        Self(BreakerIdValue::Number(value))
    }
}

impl From<&'static str> for BreakerId {
    fn from(value: &'static str) -> Self {
        Self(BreakerIdValue::String(Cow::Borrowed(value)))
    }
}

impl From<String> for BreakerId {
    fn from(value: String) -> Self {
        Self(BreakerIdValue::String(Cow::Owned(value)))
    }
}

impl From<BreakerId> for Cow<'static, str> {
    fn from(value: BreakerId) -> Self {
        match value.0 {
            BreakerIdValue::Number(n) => Cow::Owned(n.to_string()),
            BreakerIdValue::String(s) => s,
            BreakerIdValue::Hashed(_, label) => Cow::Borrowed(label),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum BreakerIdValue {
    Number(u64),
    Hashed(u64, &'static str),
    String(Cow<'static, str>),
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::Hash;

    use super::*;

    #[test]
    fn is_default_distinguishes_default_from_others() {
        assert!(BreakerId::default().is_default());
        // Any string equal to "default" is treated as the default, regardless of ownership.
        assert!(BreakerId::from(String::from("default")).is_default());

        assert!(!BreakerId::from("other").is_default());
        assert!(!BreakerId::from(42u64).is_default());
        assert!(!BreakerId::hashed(&"value", "default").is_default());
    }

    #[test]
    fn hashed_matches_manual_hasher() {
        let value = "some value";
        let id = BreakerId::hashed(&value, "my_label");

        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let expected = hasher.finish();

        match &id.0 {
            BreakerIdValue::Hashed(n, _) => {
                assert_eq!(*n, expected);
            }
            _ => panic!("Expected Inner::Hashed variant"),
        }

        assert_eq!(id.to_string(), "my_label");
    }
}
