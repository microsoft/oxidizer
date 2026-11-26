// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
enum OwnedAttributeValue {
    Int(i64),
    Double(f64),
    String(Arc<str>),
    Boolean(bool),
}

impl OwnedAttributeValue {
    fn borrowed(&self) -> AttributeValue<'_> {
        match self {
            Self::Int(value) => AttributeValue::Int(*value),
            Self::Double(value) => AttributeValue::Double(*value),
            Self::String(value) => AttributeValue::String(value),
            Self::Boolean(value) => AttributeValue::Boolean(*value),
        }
    }
}

impl<'a> From<AttributeValue<'a>> for OwnedAttributeValue {
    fn from(value: AttributeValue<'a>) -> Self {
        match value {
            AttributeValue::Int(value) => Self::Int(value),
            AttributeValue::Double(value) => Self::Double(value),
            AttributeValue::String(value) => Self::String(Arc::from(value)),
            AttributeValue::Boolean(value) => Self::Boolean(value),
        }
    }
}

/// Structured diagnostic value borrowed from an [`Attribute`].
///
/// Publish only values you can disclose in diagnostic reports. The crate does not redact
/// attribute values. Do not publish credentials, connection strings, or other sensitive data.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue<'a> {
    /// Signed integer.
    Int(i64),
    /// Floating-point number.
    Double(f64),
    /// Text.
    String(&'a str),
    /// Boolean.
    Boolean(bool),
}

impl Display for AttributeValue<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => Display::fmt(value, f),
            Self::Double(value) => Display::fmt(value, f),
            Self::String(value) => Display::fmt(value, f),
            Self::Boolean(value) => Display::fmt(value, f),
        }
    }
}

impl<'a> From<i64> for AttributeValue<'a> {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl<'a> From<f64> for AttributeValue<'a> {
    fn from(value: f64) -> Self {
        Self::Double(value)
    }
}

impl<'a> From<bool> for AttributeValue<'a> {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl<'a> From<&'a str> for AttributeValue<'a> {
    fn from(value: &'a str) -> Self {
        Self::String(value)
    }
}

/// Named diagnostic value attached to a [`Signal`](crate::Signal).
///
/// Pass `(name, value)` pairs to [`Component::publisher_with`](crate::Component::publisher_with)
/// or [`Publisher::publish`](crate::Publisher::publish) to attach attributes.
///
/// # Examples
///
/// ```
/// use oxidizer_health::{Attribute, AttributeValue};
///
/// let attribute = Attribute::from(("latency_ms", 250_i64));
///
/// assert_eq!(attribute.name(), "latency_ms");
/// assert_eq!(attribute.value(), AttributeValue::Int(250));
/// assert_eq!(attribute.to_string(), "latency_ms=250");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    name: Arc<str>,
    value: OwnedAttributeValue,
}

impl Attribute {
    /// Creates an attribute with the supplied name and typed diagnostic value.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, value: AttributeValue<'_>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    fn from_owned(name: impl Into<Arc<str>>, value: OwnedAttributeValue) -> Self {
        Self { name: name.into(), value }
    }

    /// Borrows the name supplied for this attribute.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrows the typed value without formatting it as text.
    #[must_use]
    pub fn value(&self) -> AttributeValue<'_> {
        self.value.borrowed()
    }
}

impl Display for Attribute {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.value.borrowed())
    }
}

impl<'a, N> From<(N, AttributeValue<'a>)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, AttributeValue<'a>)) -> Self {
        Self::new(name, value)
    }
}

impl<N> From<(N, i64)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, i64)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::Int(value))
    }
}

impl<N> From<(N, f64)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, f64)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::Double(value))
    }
}

impl<N> From<(N, bool)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, bool)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::Boolean(value))
    }
}

impl<'a, N> From<(N, &'a str)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, &'a str)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::String(Arc::from(value)))
    }
}

impl<N> From<(N, String)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, String)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::String(Arc::from(value)))
    }
}

impl<N> From<(N, Box<str>)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, Box<str>)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::String(Arc::from(value)))
    }
}

impl<N> From<(N, Arc<str>)> for Attribute
where
    N: Into<Arc<str>>,
{
    fn from((name, value): (N, Arc<str>)) -> Self {
        Self::from_owned(name, OwnedAttributeValue::String(value))
    }
}
