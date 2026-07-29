// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::String;
use core::fmt;
use core::str::FromStr;

use crate::ConfigurationError;

/// An HTTP method a route matches on.
///
/// Passed to a generated route builder's `add_<variant>` methods to register
/// a dynamic route's method. Matching is by exact, case-sensitive token (its
/// [`as_str`](Self::as_str)). Use one of the standard constants or
/// [`custom`](Self::custom) for an extension method such as `M-SEARCH`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HttpMethod(HttpMethodRepr);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum HttpMethodRepr {
    Standard(&'static str),
    Custom(String),
}

impl HttpMethod {
    /// `GET`.
    pub const GET: Self = Self(HttpMethodRepr::Standard("GET"));
    /// `PUT`.
    pub const PUT: Self = Self(HttpMethodRepr::Standard("PUT"));
    /// `POST`.
    pub const POST: Self = Self(HttpMethodRepr::Standard("POST"));
    /// `DELETE`.
    pub const DELETE: Self = Self(HttpMethodRepr::Standard("DELETE"));
    /// `PATCH`.
    pub const PATCH: Self = Self(HttpMethodRepr::Standard("PATCH"));
    /// `HEAD`.
    pub const HEAD: Self = Self(HttpMethodRepr::Standard("HEAD"));
    /// `OPTIONS`.
    pub const OPTIONS: Self = Self(HttpMethodRepr::Standard("OPTIONS"));
    /// `CONNECT`.
    pub const CONNECT: Self = Self(HttpMethodRepr::Standard("CONNECT"));
    /// `TRACE`.
    pub const TRACE: Self = Self(HttpMethodRepr::Standard("TRACE"));

    /// Creates an HTTP method after validating its RFC 9110 token syntax.
    ///
    /// Standard method names produce the corresponding standard constant.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError`] when `value` is empty or contains a byte
    /// that is not allowed in an RFC 9110 `token`.
    ///
    /// # Examples
    ///
    /// ```
    /// use routerama::HttpMethod;
    ///
    /// # fn main() -> Result<(), routerama::ConfigurationError> {
    /// let method = HttpMethod::custom("M-SEARCH")?;
    /// assert_eq!(method.as_str(), "M-SEARCH");
    /// assert_eq!(HttpMethod::custom("GET")?, HttpMethod::GET);
    /// # Ok(())
    /// # }
    /// ```
    pub fn custom(value: impl AsRef<str>) -> Result<Self, ConfigurationError> {
        let value = value.as_ref();
        if !routerama_build::is_http_token(value) {
            return Err(ConfigurationError::invalid_http_method(value.into()));
        }
        Ok(Self::standard(value).unwrap_or_else(|| Self(HttpMethodRepr::Custom(value.into()))))
    }

    fn from_owned(value: String) -> Result<Self, ConfigurationError> {
        if !routerama_build::is_http_token(&value) {
            return Err(ConfigurationError::invalid_http_method(value));
        }
        Ok(Self::standard(&value).unwrap_or(Self(HttpMethodRepr::Custom(value))))
    }

    fn standard(value: &str) -> Option<Self> {
        match value {
            "GET" => Some(Self::GET),
            "PUT" => Some(Self::PUT),
            "POST" => Some(Self::POST),
            "DELETE" => Some(Self::DELETE),
            "PATCH" => Some(Self::PATCH),
            "HEAD" => Some(Self::HEAD),
            "OPTIONS" => Some(Self::OPTIONS),
            "CONNECT" => Some(Self::CONNECT),
            "TRACE" => Some(Self::TRACE),
            _ => None,
        }
    }

    /// The validated HTTP method token used for matching.
    ///
    /// # Examples
    ///
    /// ```
    /// use routerama::HttpMethod;
    ///
    /// # fn main() -> Result<(), routerama::ConfigurationError> {
    /// assert_eq!(HttpMethod::GET.as_str(), "GET");
    /// assert_eq!(HttpMethod::custom("M-SEARCH")?.as_str(), "M-SEARCH");
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            HttpMethodRepr::Standard(method) => method,
            HttpMethodRepr::Custom(method) => method,
        }
    }
}

impl FromStr for HttpMethod {
    type Err = ConfigurationError;

    fn from_str(method: &str) -> Result<Self, Self::Err> {
        Self::custom(method)
    }
}

impl TryFrom<&str> for HttpMethod {
    type Error = ConfigurationError;

    fn try_from(method: &str) -> Result<Self, Self::Error> {
        method.parse()
    }
}

impl TryFrom<String> for HttpMethod {
    type Error = ConfigurationError;

    fn try_from(method: String) -> Result<Self, Self::Error> {
        Self::from_owned(method)
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for HttpMethod {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<HttpMethod> for String {
    fn from(method: HttpMethod) -> Self {
        match method.0 {
            HttpMethodRepr::Standard(method) => method.into(),
            HttpMethodRepr::Custom(method) => method,
        }
    }
}
