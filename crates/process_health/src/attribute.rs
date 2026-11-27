// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

/// Named diagnostic text attached to a [`Signal`](crate::Signal).
///
/// Pass `(name, value)` pairs to [`Component::publisher_with`](crate::Component::publisher_with)
/// or [`Publisher::publish`](crate::Publisher::publish) to attach attributes.
///
/// Publish only values you can disclose in diagnostic reports. The crate does not redact
/// attribute values. Do not publish credentials, connection strings, or other sensitive data.
///
/// # Examples
///
/// ```
/// use health::Attribute;
///
/// let attribute = Attribute::from(("latency_ms", "250"));
///
/// assert_eq!(attribute.name(), "latency_ms");
/// assert_eq!(attribute.value(), "250");
/// assert_eq!(attribute.to_string(), "latency_ms=250");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    name: Arc<str>,
    value: Arc<str>,
}

impl Attribute {
    /// Creates an attribute with the supplied name and diagnostic text.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, value: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Borrows the attribute name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrows the diagnostic text.
    #[must_use]
    pub fn value(&self) -> &str {
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
    N: Into<Arc<str>>,
    V: Into<Arc<str>>,
{
    fn from((name, value): (N, V)) -> Self {
        Self::new(name, value)
    }
}
