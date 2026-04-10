// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;
use std::io::ErrorKind;

/// A low-cardinality label for an [`HttpError`](crate::HttpError), useful for metrics and logging.
///
/// Wraps a [`Cow<'static, str>`] so it can hold either a static string literal
/// or a heap-allocated [`String`].
///
/// # Examples
///
/// ```
/// # use http_extensions::HttpErrorLabel;
/// // From a static string
/// let label: HttpErrorLabel = "timeout".into();
/// assert_eq!(label, "timeout");
///
/// // From an owned String
/// let label: HttpErrorLabel = String::from("custom_label").into();
/// assert_eq!(label, "custom_label");
/// ```
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct HttpErrorLabel(Cow<'static, str>);

impl HttpErrorLabel {
    /// Creates a label by joining the parts with `.` as a separator.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpErrorLabel;
    /// let label = HttpErrorLabel::with_parts(["http", "timeout"]);
    /// assert_eq!(label, "http.timeout");
    ///
    /// let label = HttpErrorLabel::with_parts(["a", "b", "c"]);
    /// assert_eq!(label, "a.b.c");
    /// ```
    #[must_use]
    pub fn with_parts(parts: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut parts = parts.into_iter();
        let mut result = match parts.next() {
            Some(first) => String::from(first.as_ref()),
            None => return Self(Cow::Borrowed("")),
        };
        for part in parts {
            result.push('.');
            result.push_str(part.as_ref());
        }
        Self(Cow::Owned(result))
    }

    /// Returns the label as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HttpErrorLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for HttpErrorLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<&'static str> for HttpErrorLabel {
    fn from(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }
}

impl From<String> for HttpErrorLabel {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s))
    }
}

impl From<Cow<'static, str>> for HttpErrorLabel {
    fn from(s: Cow<'static, str>) -> Self {
        Self(s)
    }
}

impl PartialEq<str> for HttpErrorLabel {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for HttpErrorLabel {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl AsRef<str> for HttpErrorLabel {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Default for HttpErrorLabel {
    fn default() -> Self {
        Self(Cow::Borrowed(""))
    }
}

impl From<ErrorKind> for HttpErrorLabel {
    fn from(s: ErrorKind) -> Self {
        Self(Cow::Owned(s.to_string()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn from_static_str() {
        let label = HttpErrorLabel::from("static_label");
        assert_eq!(label, "static_label");
        assert_eq!(label.as_str(), "static_label");
    }

    #[test]
    fn from_string() {
        let label = HttpErrorLabel::from(String::from("owned_label"));
        assert_eq!(label, "owned_label");
        assert_eq!(label.as_str(), "owned_label");
    }

    #[test]
    fn from_cow() {
        let cow: Cow<'static, str> = Cow::Owned(String::from("cow_label"));
        let label = HttpErrorLabel::from(cow);
        assert_eq!(label, "cow_label");
    }

    #[test]
    fn display() {
        let label = HttpErrorLabel::from("display_test");
        assert_eq!(format!("{label}"), "display_test");
    }

    #[test]
    fn debug() {
        let label = HttpErrorLabel::from("debug_test");
        assert_eq!(format!("{label:?}"), "\"debug_test\"");
    }

    #[test]
    fn clone_and_eq() {
        let label = HttpErrorLabel::from("clone_test");
        let cloned = label.clone();
        assert_eq!(label, cloned);
    }

    #[test]
    fn default_is_empty() {
        let label = HttpErrorLabel::default();
        assert_eq!(label, "");
    }

    #[test]
    fn as_ref_str() {
        let label = HttpErrorLabel::from("as_ref_test");
        let s: &str = label.as_ref();
        assert_eq!(s, "as_ref_test");
    }

    #[test]
    fn with_parts_multiple() {
        let label = HttpErrorLabel::with_parts(["http", "client", "timeout"]);
        assert_eq!(label, "http.client.timeout");
    }

    #[test]
    fn with_parts_single() {
        let label = HttpErrorLabel::with_parts(["only"]);
        assert_eq!(label, "only");
    }

    #[test]
    fn with_parts_empty() {
        let label = HttpErrorLabel::with_parts(std::iter::empty::<&str>());
        assert_eq!(label, "");
    }

    #[test]
    fn with_parts_owned_strings() {
        let parts = vec![String::from("a"), String::from("b")];
        let label = HttpErrorLabel::with_parts(parts);
        assert_eq!(label, "a.b");
    }
}
