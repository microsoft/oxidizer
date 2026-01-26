// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Display;
use std::hash::Hasher;

/// Identifies an isolated circuit breaker instance.
///
/// Each unique `BreakerId` maintains its own independent circuit breaker state, including
/// failure counts, health metrics, and open/closed status. This allows you to have separate
/// circuit breakers for different backends, services, or logical groupings of inputs.
///
/// Breaker IDs should be **long-lived and low-cardinality**, representing distinct failure
/// domains (e.g., backend hosts, service endpoints). Avoid high-cardinality IDs like user IDs
/// or request IDs—these cause unbounded memory growth and prevent detection of systemic failures.
///
/// For maximum performance, prefer integer IDs or static string IDs (`&'static str`).
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
/// // Simulate extracting authority and scheme from an HTTP request
/// let scheme = "https";
/// let authority = "api.example.com";
/// let id_value = format!("{}://{}", scheme, authority);
///
/// let id = BreakerId::from(id_value);
/// assert_eq!(id.to_string(), "https://api.example.com");
///
/// // For better performance, use hashing. Note that you must provide a display label
/// // for the hashed ID.
/// let hashed_id = BreakerId::hashed(&(scheme, authority), "scheme_and_authority");
/// assert_eq!(hashed_id.to_string(), "scheme_and_authority");
/// ```
///
/// # Telemetry
///
/// The values used to create breaker IDs are included in telemetry data (logs and metrics)
/// for observability purposes. **Important**: Ensure that the values from which breaker IDs
/// are created do not contain any sensitive data such as authentication tokens, personal
/// identifiable information (PII), or other confidential data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BreakerId(BreakerIdValue);

impl BreakerId {
    pub(crate) const fn default() -> Self {
        Self(BreakerIdValue::String(Cow::Borrowed("default")))
    }

    /// Create a breaker ID by hashing the given value.
    ///
    /// The value must implement the `Hash` trait. This is useful for creating breaker
    /// IDs from complex types. The resulting breaker ID will be based on the hash of
    /// the value. You must provide a `label` that will be used for display and telemetry.
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum BreakerIdValue {
    Number(u64),
    Hashed(u64, &'static str),
    String(Cow<'static, str>),
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::hash::Hash;

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(BreakerId: Send, Sync, Unpin, Clone, Hash, Display, Debug, PartialEq, Eq);

    #[test]
    fn from_u64_and_display() {
        let k = BreakerId::from(42u64);
        assert_eq!(k.to_string(), "42");
        assert_eq!(k, BreakerId::from(42u64));
    }

    #[test]
    fn from_static_str_and_string() {
        let a: BreakerId = "hello".into();
        let b: BreakerId = String::from("hello").into();
        assert_eq!(a.to_string(), "hello");
        assert_eq!(b.to_string(), "hello");
        assert_eq!(a, b);
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

    #[test]
    fn breaker_id_hash_consistent() {
        let k1 = BreakerId::from(123u64);
        let k2 = BreakerId::from(123u64);

        let mut h1 = DefaultHasher::new();
        k1.hash(&mut h1);
        let mut h2 = DefaultHasher::new();
        k2.hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn into_cow_from_number() {
        let id = BreakerId::from(42u64);
        let cow: Cow<'static, str> = id.into();
        assert!(matches!(cow, Cow::Owned(_)));
        assert_eq!(cow, "42");
    }

    #[test]
    fn into_cow_from_string_owned() {
        let id = BreakerId::from(String::from("owned_string"));
        let cow: Cow<'static, str> = id.into();
        assert!(matches!(cow, Cow::Owned(_)));
        assert_eq!(cow, "owned_string");
    }

    #[test]
    fn into_cow_from_string_borrowed() {
        let id = BreakerId::from("static_str");
        let cow: Cow<'static, str> = id.into();
        assert!(matches!(cow, Cow::Borrowed(_)));
        assert_eq!(cow, "static_str");
    }

    #[test]
    fn into_cow_from_hashed() {
        let id = BreakerId::hashed(&"value", "label");
        let cow: Cow<'static, str> = id.into();
        assert!(matches!(cow, Cow::Borrowed(_)));
        assert_eq!(cow, "label");
    }
}
