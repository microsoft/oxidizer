// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt::{self, Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Immutable string used for attribute names and string values.
///
/// Static strings are stored without allocation. Other strings are stored in shared memory, so
/// cloning never copies the text.
///
/// # Examples
///
/// ```
/// use oxidizer_health::AttributeString;
///
/// const REASON: AttributeString = AttributeString::new("reason");
///
/// assert_eq!(REASON.as_str(), "reason");
/// assert_eq!(AttributeString::from(String::from("reason")), REASON);
/// ```
#[derive(Clone)]
pub struct AttributeString(Repr);

#[derive(Clone)]
enum Repr {
    Static(&'static str),
    Shared(Arc<str>),
}

impl AttributeString {
    /// Creates an attribute string from static text without allocating.
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(Repr::Static(value))
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Static(value) => value,
            Repr::Shared(value) => value,
        }
    }
}

impl AsRef<str> for AttributeString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for AttributeString {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Debug for AttributeString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}

impl Display for AttributeString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq for AttributeString {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for AttributeString {}

// Hashing the text keeps `Hash` consistent with `Borrow<str>`.
impl Hash for AttributeString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialOrd for AttributeString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AttributeString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl From<&str> for AttributeString {
    fn from(value: &str) -> Self {
        Self(Repr::Shared(Arc::from(value)))
    }
}

impl From<String> for AttributeString {
    fn from(value: String) -> Self {
        Self(Repr::Shared(Arc::from(value)))
    }
}

impl From<Box<str>> for AttributeString {
    fn from(value: Box<str>) -> Self {
        Self(Repr::Shared(Arc::from(value)))
    }
}

/// Structured diagnostic value of an [`Attribute`].
///
/// Attributes are diagnostic-safe values. Do not publish credentials, connection strings,
/// unrestricted host data, or other sensitive values.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue {
    /// Signed integer.
    Int(i64),
    /// Floating-point number.
    Double(f64),
    /// Text.
    String(AttributeString),
    /// Boolean.
    Boolean(bool),
}

impl Display for AttributeValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => Display::fmt(value, f),
            Self::Double(value) => Display::fmt(value, f),
            Self::String(value) => Display::fmt(value, f),
            Self::Boolean(value) => Display::fmt(value, f),
        }
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        Self::Double(value)
    }
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        Self::String(value.into())
    }
}

impl From<Box<str>> for AttributeValue {
    fn from(value: Box<str>) -> Self {
        Self::String(value.into())
    }
}

/// Named diagnostic value attached to a [`Signal`](crate::Signal).
///
/// # Examples
///
/// ```
/// use oxidizer_health::{Attribute, AttributeValue};
///
/// let attribute = Attribute::from(("latency_ms", 250_i64));
///
/// assert_eq!(attribute.name().as_str(), "latency_ms");
/// assert_eq!(attribute.value(), &AttributeValue::Int(250));
/// assert_eq!(attribute.to_string(), "latency_ms=250");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    name: AttributeString,
    value: AttributeValue,
}

impl Attribute {
    /// Creates an attribute.
    #[must_use]
    pub const fn new(name: AttributeString, value: AttributeValue) -> Self {
        Self { name, value }
    }

    /// Returns the attribute name.
    #[must_use]
    pub const fn name(&self) -> &AttributeString {
        &self.name
    }

    /// Returns the attribute value.
    #[must_use]
    pub const fn value(&self) -> &AttributeValue {
        &self.value
    }
}

impl Display for Attribute {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.value)
    }
}

impl<N, V> From<(N, V)> for Attribute
where
    N: Into<AttributeString>,
    V: Into<AttributeValue>,
{
    fn from((name, value): (N, V)) -> Self {
        Self::new(name.into(), value.into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn static_and_shared_strings_compare_and_hash_by_text() {
        let shared = AttributeString::from(Box::<str>::from("name"));
        assert_eq!(shared, AttributeString::new("name"));
        assert!(AttributeString::new("a") < AttributeString::from("b"));

        let set = HashSet::from([shared]);
        assert!(set.contains("name"));
    }

    #[test]
    fn values_display_their_payload() {
        assert_eq!(AttributeValue::from(1.5).to_string(), "1.5");
        assert_eq!(AttributeValue::from(true).to_string(), "true");
        assert_eq!(AttributeValue::from(String::from("text")).to_string(), "text");
    }
}
