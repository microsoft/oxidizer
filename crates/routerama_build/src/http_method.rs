// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

/// An HTTP method a route matches on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HttpMethod {
    /// `GET`.
    Get,
    /// `PUT`.
    Put,
    /// `POST`.
    Post,
    /// `DELETE`.
    Delete,
    /// `PATCH`.
    Patch,
    /// A custom method, carrying an arbitrary HTTP method name (e.g. `HEAD`).
    Custom(String),
}

impl HttpMethod {
    /// The upper-case HTTP method token used for matching, e.g. `"GET"`.
    ///
    /// # Examples
    ///
    /// ```
    /// use routerama_build::HttpMethod;
    ///
    /// assert_eq!(HttpMethod::Get.as_str(), "GET");
    /// assert_eq!(HttpMethod::Custom("HEAD".to_owned()).as_str(), "HEAD");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Custom(kind) => kind.as_str(),
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for HttpMethod {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_as_ref_match_as_str() {
        assert_eq!(HttpMethod::Post.to_string(), "POST");
        assert_eq!(HttpMethod::Custom("HEAD".to_owned()).to_string(), "HEAD");
        assert_eq!(HttpMethod::Delete.as_ref() as &str, "DELETE");
    }

    #[test]
    fn method_tokens_cover_standard_methods() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
    }
}
