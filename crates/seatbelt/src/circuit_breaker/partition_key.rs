// Copyright (c) Microsoft Corporation.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Display;
use std::hash::Hasher;

/// Key that identifies a partition for which a separate circuit breaker instance is maintained.
///
/// Currently, it supports either integer or string keys. For maximum performance, prefer using integer keys
/// or static string keys (i.e. `&'static str`).
///
/// # Examples
///
/// ## Creation from a number
///
/// ```rust
/// use seatbelt::circuit_breaker::PartitionKey;
///
/// let key = PartitionKey::from(42_u64);
/// assert_eq!(key.to_string(), "42");
/// ```
///
/// ## Creation from HTTP request authority and scheme
///
/// ```rust
/// use seatbelt::circuit_breaker::PartitionKey;
///
/// // Simulate extracting authority and scheme from an HTTP request
/// let scheme = "https";
/// let authority = "api.example.com";
/// let partition_value = format!("{}://{}", scheme, authority);
///
/// let key = PartitionKey::from(partition_value);
/// assert_eq!(key.to_string(), "https://api.example.com");
///
/// // For better performance, use hashing. Note that you must provide a display label
/// // for the hashed key.
/// let hashed_key = PartitionKey::hashed(&(scheme, authority), "scheme_and_authority");
/// assert_eq!(hashed_key.to_string(), "scheme_and_authority");
/// ```
///
/// # Telemetry
///
/// The values used to create partition keys are included in telemetry data (logs and metrics)
/// for observability purposes. **Important**: Ensure that the values from which partition keys
/// are created do not contain any sensitive data such as authentication tokens, personal
/// identifiable information (PII), or other confidential data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartitionKey(PartitionKeyValue);

impl PartitionKey {
    pub(crate) const fn default() -> Self {
        Self(PartitionKeyValue::String(Cow::Borrowed("default")))
    }

    /// Create a partition key by hashing the given value.
    ///
    /// The value must implement the `Hash` trait. This is useful for creating partition
    /// keys from complex types. The resulting partition key will be based on the hash of
    /// the value. You must provide a `label` that will be used for display and telemetry.
    pub fn hashed<T: std::hash::Hash>(value: &T, label: &'static str) -> Self {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        Self(PartitionKeyValue::Hashed(hasher.finish(), label))
    }
}

impl Display for PartitionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            PartitionKeyValue::Number(n) => write!(f, "{n}"),
            PartitionKeyValue::String(s) => f.write_str(s),
            PartitionKeyValue::Hashed(_, label) => f.write_str(label),
        }
    }
}

impl From<u64> for PartitionKey {
    fn from(value: u64) -> Self {
        Self(PartitionKeyValue::Number(value))
    }
}

impl From<&'static str> for PartitionKey {
    fn from(value: &'static str) -> Self {
        Self(PartitionKeyValue::String(Cow::Borrowed(value)))
    }
}

impl From<String> for PartitionKey {
    fn from(value: String) -> Self {
        Self(PartitionKeyValue::String(Cow::Owned(value)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PartitionKeyValue {
    Number(u64),
    Hashed(u64, &'static str),
    String(Cow<'static, str>),
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::hash::Hash;

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(PartitionKey: Send, Sync, Unpin, Clone, Hash, Display, Debug, PartialEq, Eq);

    #[test]
    fn from_u64_and_display() {
        let k = PartitionKey::from(42u64);
        assert_eq!(k.to_string(), "42");
        assert_eq!(k, PartitionKey::from(42u64));
    }

    #[test]
    fn from_static_str_and_string() {
        let a: PartitionKey = "hello".into();
        let b: PartitionKey = String::from("hello").into();
        assert_eq!(a.to_string(), "hello");
        assert_eq!(b.to_string(), "hello");
        assert_eq!(a, b);
    }

    #[test]
    fn hashed_matches_manual_hasher() {
        let value = "some value";
        let pk = PartitionKey::hashed(&value, "my_label");

        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let expected = hasher.finish();

        match &pk.0 {
            PartitionKeyValue::Hashed(n, _) => {
                assert_eq!(*n, expected);
            }
            _ => panic!("Expected Inner::Hashed variant"),
        }

        assert_eq!(pk.to_string(), "my_label");
    }

    #[test]
    fn partitionkey_hash_consistent() {
        let k1 = PartitionKey::from(123u64);
        let k2 = PartitionKey::from(123u64);

        let mut h1 = DefaultHasher::new();
        k1.hash(&mut h1);
        let mut h2 = DefaultHasher::new();
        k2.hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }
}
