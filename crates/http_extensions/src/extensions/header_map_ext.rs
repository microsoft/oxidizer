// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::str::FromStr;

use http::HeaderMap;
use http::header::AsHeaderName;

/// Header value extraction and parsing.
///
/// Provides methods to extract header values as strings and parse them into typed values.
pub trait HeaderMapExt: sealed::Sealed {
    /// Gets a header value as a string slice.
    ///
    /// Returns `None` if the header is not present or not valid UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderMap, HeaderValue};
    /// use http_extensions::HeaderMapExt;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("Server", HeaderValue::from_static("oxidizer/1.0"));
    ///
    /// let server = headers.get_str_value("Server");
    /// assert_eq!(server, Some("oxidizer/1.0"));
    ///
    /// // Non-existent header
    /// let missing = headers.get_str_value("X-Missing");
    /// assert_eq!(missing, None);
    /// ```
    fn get_str_value(&self, header_name: impl AsHeaderName) -> Option<&str>;

    /// Gets a header value as a string slice, or returns a default value.
    ///
    /// Returns the default if the header is not present or not valid UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderMap, HeaderValue};
    /// use http_extensions::HeaderMapExt;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("Content-Language", HeaderValue::from_static("en-US"));
    ///
    /// // Existing header
    /// let language = headers.get_str_value_or("Content-Language", "en");
    /// assert_eq!(language, "en-US");
    ///
    /// // Missing header - returns default value
    /// let region = headers.get_str_value_or("X-Region", "us-west");
    /// assert_eq!(region, "us-west");
    /// ```
    fn get_str_value_or<'a>(&'a self, header_name: impl AsHeaderName, default: &'a str) -> &'a str {
        self.get_str_value(header_name).unwrap_or(default)
    }

    /// Gets a header value parsed to a specific type.
    ///
    /// Returns `None` if the header is not present, not valid UTF-8, or cannot be parsed.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderMap, HeaderValue};
    /// use http_extensions::HeaderMapExt;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("X-Rate-Limit", HeaderValue::from(100));
    ///
    /// // Parse to the appropriate type
    /// let limit: u32 = headers.get_value("X-Rate-Limit").unwrap();
    /// assert_eq!(limit, 100);
    ///
    /// // Missing headers return None
    /// let missing: Option<u32> = headers.get_value("X-Missing");
    /// assert_eq!(missing, None);
    ///
    /// // Unparsable values return None
    /// headers.insert("X-Bad-Integer", "not-a-number".parse().unwrap());
    /// let bad_number: Option<i32> = headers.get_value("X-Bad-Integer");
    /// assert_eq!(bad_number, None);
    /// ```
    fn get_value<T: FromStr>(&self, header_name: impl AsHeaderName) -> Option<T>;

    /// Gets a header value parsed to a specific type, or returns a default value.
    ///
    /// Returns the default if the header is not present, not valid UTF-8, or cannot be parsed.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderMap, HeaderValue};
    /// use http_extensions::HeaderMapExt;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("X-Timeout", HeaderValue::from(30));
    /// headers.insert("X-Invalid", HeaderValue::from_static("invalid"));
    ///
    /// // Parse value with fallback
    /// let timeout: u32 = headers.get_value_or("X-Timeout", 60);
    /// assert_eq!(timeout, 30);
    ///
    /// // Missing header returns default
    /// let retry: u32 = headers.get_value_or("X-Retry-Count", 3);
    /// assert_eq!(retry, 3);
    ///
    /// // Unparsable value returns default
    /// let invalid: f32 = headers.get_value_or("X-Invalid", 1.0);
    /// assert_eq!(invalid, 1.0);
    /// ```
    fn get_value_or<T: FromStr>(&self, header_name: impl AsHeaderName, default: T) -> T {
        self.get_value(header_name).unwrap_or(default)
    }
}

impl HeaderMapExt for HeaderMap {
    fn get_str_value(&self, header_name: impl AsHeaderName) -> Option<&str> {
        self.get(header_name).and_then(|v| v.to_str().ok())
    }

    fn get_value<T: FromStr>(&self, header_name: impl AsHeaderName) -> Option<T> {
        self.get(header_name).and_then(|v| v.to_str().ok()).and_then(|str| str.parse().ok())
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl Sealed for HeaderMap {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn get_str_value() {
        let mut headers = HeaderMap::new();

        headers.insert("XYZ", "dummy".parse().unwrap());

        assert_eq!(headers.get_str_value_or("XYZ", ""), "dummy");
        assert_eq!(headers.get_str_value_or("does_not_exist", "def"), "def");
    }

    #[test]
    fn get_value_ok() {
        let mut headers = HeaderMap::new();
        headers.insert("XYZ", "10".parse().unwrap());
        assert_eq!(headers.get_value_or("XYZ", 0), 10);
    }
    #[test]
    fn get_value_invalid_returns_default() {
        let mut headers = HeaderMap::new();
        headers.insert("XYZ", "abc".parse().unwrap());
        assert_eq!(headers.get_value_or("XYZ", 4), 4);
    }
}
