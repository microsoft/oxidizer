// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

/// A label for the URL template that can be attached to HTTP requests as an extension.
///
/// This type is a **workaround** for cases where a templated URI cannot be used.
/// When possible, prefer using templated URIs via the `#[templated]` macro from
/// `templated_uri`, as they provide better type safety and automatic template extraction.
///
/// When both a templated URI and a `UrlTemplateLabel` are present on a request,
/// the explicit `UrlTemplateLabel` takes precedence.
///
/// # Example
///
/// ```
/// use http_extensions::{HttpRequestBuilder, UrlTemplateLabel};
///
/// let request = HttpRequestBuilder::new_fake()
///     .get("https://example.com/api/users/123")
///     .extension(UrlTemplateLabel::new("/api/users/{id}"))
///     .build()
///     .unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct UrlTemplateLabel(Cow<'static, str>);

impl UrlTemplateLabel {
    /// Creates a new `UrlTemplateLabel` from any type that can be converted to a `Cow<'static, str>`.
    ///
    /// This accepts static strings, owned `String`s, or `Cow<'static, str>` directly.
    #[must_use]
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self(label.into())
    }

    /// Creates a new `UrlTemplateLabel` from a static string.
    ///
    /// This is a const function that can be used in const contexts.
    #[must_use]
    pub const fn new_static(label: &'static str) -> Self {
        Self(Cow::Borrowed(label))
    }

    /// Returns the label as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert the template into `Cow<'static, str>`.
    #[must_use]
    pub fn into_cow(self) -> Cow<'static, str> {
        self.0
    }
}

impl std::fmt::Display for UrlTemplateLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for UrlTemplateLabel {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for UrlTemplateLabel {
    fn from(label: &'static str) -> Self {
        Self::new_static(label)
    }
}

impl From<String> for UrlTemplateLabel {
    fn from(label: String) -> Self {
        Self::new(label)
    }
}

impl From<UrlTemplateLabel> for Cow<'static, str> {
    fn from(label: UrlTemplateLabel) -> Self {
        label.into_cow()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn new_with_static_str() {
        let label = UrlTemplateLabel::new("/api/users/{id}");
        assert_eq!(label.as_str(), "/api/users/{id}");
    }

    #[test]
    fn new_with_string() {
        let label = UrlTemplateLabel::new("/api/users/{id}".to_string());
        assert_eq!(label.as_str(), "/api/users/{id}");
    }

    #[test]
    fn new_with_cow() {
        let cow: Cow<'static, str> = Cow::Owned("/api/users/{id}".to_string());
        let label = UrlTemplateLabel::new(cow);
        assert_eq!(label.as_str(), "/api/users/{id}");
    }

    #[test]
    fn new_static_creates_borrowed_cow() {
        let label = UrlTemplateLabel::new_static("/api/users/{id}");
        assert_eq!(label.as_str(), "/api/users/{id}");
        assert!(matches!(label.0, Cow::Borrowed(_)));
    }

    #[test]
    fn into_cow_via_from() {
        let label = UrlTemplateLabel::new("/api/users/{id}");
        let cow: Cow<'static, str> = label.into();
        assert_eq!(cow, "/api/users/{id}");
    }

    #[test]
    fn from_static_str() {
        let label: UrlTemplateLabel = "/api/users/{id}".into();
        assert_eq!(label.as_str(), "/api/users/{id}");
    }

    #[test]
    fn from_string() {
        let label: UrlTemplateLabel = "/api/users/{id}".to_string().into();
        assert_eq!(label.as_str(), "/api/users/{id}");
    }

    #[test]
    fn display_impl() {
        let label = UrlTemplateLabel::new("/api/users/{id}");
        assert_eq!(format!("{label}"), "/api/users/{id}");
    }

    #[test]
    fn as_ref_str_impl() {
        let label = UrlTemplateLabel::new("/api/users/{id}");
        let s: &str = label.as_ref();
        assert_eq!(s, "/api/users/{id}");
    }
}
