// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::{Borrow, Cow};
use std::error::Error;
use std::fmt;
use std::fmt::{Debug, Display};
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize};

use uuid::Uuid;

mod private {
    use data_privacy::Sensitive;

    use super::{IpAddr, NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize, UriSafeString, Uuid};

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}

    impl Sealed for UriSafeString {}
    impl Sealed for usize {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
    impl Sealed for u128 {}
    impl Sealed for NonZeroU8 {}
    impl Sealed for NonZeroU16 {}
    impl Sealed for NonZeroU32 {}
    impl Sealed for NonZeroU64 {}
    impl Sealed for NonZeroU128 {}
    impl Sealed for NonZeroUsize {}
    impl Sealed for IpAddr {}
    impl Sealed for Uuid {}
    impl<T> Sealed for Sensitive<T> where T: Sealed {}
    impl<T> Sealed for &T where T: Sealed + ?Sized {}
}

/// Marks types that, when [`Display`ed](std::fmt::Display), are valid for URI use.
pub trait UriSafe: private::Sealed + Display + Debug {}

impl UriSafe for UriSafeString {}
impl UriSafe for usize {}
impl UriSafe for u8 {}
impl UriSafe for u16 {}
impl UriSafe for u32 {}
impl UriSafe for u64 {}
impl UriSafe for u128 {}
impl UriSafe for NonZeroU8 {}
impl UriSafe for NonZeroU16 {}
impl UriSafe for NonZeroU32 {}
impl UriSafe for NonZeroU64 {}
impl UriSafe for NonZeroU128 {}
impl UriSafe for NonZeroUsize {}
impl UriSafe for IpAddr {}
impl UriSafe for Uuid {}
impl<T> UriSafe for &T where T: UriSafe + ?Sized {}

/// Error returned when a string contains characters that are not safe for URI templates.
#[derive(Debug)]
pub struct UriSafeError {
    /// The invalid character that was found.
    pub invalid_char: char,
    /// The position in the string where the invalid character was found.
    pub position: usize,
}

impl fmt::Display for UriSafeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid character '{}' at position {} for URI safe string",
            self.invalid_char, self.position
        )
    }
}

impl Error for UriSafeError {}

/// A wrapper around String that guarantees the inner value is safe to use in URI templates.
///
/// According to RFC 6570, certain characters are reserved and must be percent-encoded.
/// This type ensures its content doesn't contain those reserved characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UriSafeString(Cow<'static, str>);

impl UriSafeString {
    /// Creates a new `UriSafeString` if the provided string doesn't contain
    /// any reserved characters as defined in RFC 6570.
    ///
    /// # Returns
    ///
    /// Returns a Result containing either the `UriSafeString` or an `UriSafeError`
    /// indicating which character is invalid and where it was found.
    ///
    /// # Examples
    ///
    /// ```
    /// use obscuri::UriSafeString;
    ///
    /// let safe = UriSafeString::new(&"hello_world");
    /// assert!(safe.is_ok());
    ///
    /// let unsafe_str = UriSafeString::new(&"{hello}");
    /// assert!(unsafe_str.is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`UriSafeError`] if the string contains reserved URI characters.
    pub fn new(s: &impl ToString) -> Result<Self, UriSafeError> {
        let s = s.to_string();
        // Check for reserved characters according to RFC 6570
        for (i, c) in s.char_indices() {
            // Reserved characters in RFC 6570:
            // - Template delimiters: '{', '}'
            // - Reserved gen-delims (RFC 3986): ':', '/', '?', '#', '[', ']', '@'
            // - Reserved sub-delims (RFC 3986): '!', '$', '&', '\'', '(', ')', '*', '+', ',', ';', '='
            if "{}/:?#[]@!$&'()*+,;=".contains(c) {
                return Err(UriSafeError {
                    invalid_char: c,
                    position: i,
                });
            }
        }
        Ok(Self(Cow::Owned(s)))
    }

    /// Returns a reference to the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow()
    }

    /// Creates a `UriSafeString` from a string literal, verifying at compile time
    /// that the string does not contain any reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use obscuri::UriSafeString;
    ///
    /// // This will compile successfully
    /// let safe = UriSafeString::from_static("hello_world");
    ///
    /// // This would fail to compile:
    /// // let unsafe_str = UriSafeString::from_static("{hello}");
    /// ```
    ///
    /// # Panics
    /// if the provided string contains any reserved characters.
    #[cfg_attr(test, mutants::skip)] // Mutating this function leads to infinite loop and timeout
    #[expect(clippy::panic, reason = "panic is intentional for compile-time validation")]
    #[inline]
    #[must_use]
    pub const fn from_static(s: &'static str) -> Self {
        // Use the same validation logic as in uri_safe! macro
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            // Check for ASCII characters that are reserved
            if b == b'{'
                || b == b'}'
                || b == b'/'
                || b == b':'
                || b == b'?'
                || b == b'#'
                || b == b'['
                || b == b']'
                || b == b'@'
                || b == b'!'
                || b == b'$'
                || b == b'&'
                || b == b'\''
                || b == b'('
                || b == b')'
                || b == b'*'
                || b == b'+'
                || b == b','
                || b == b';'
                || b == b'='
            {
                // This will trigger a compile-time panic if an invalid character is found
                panic!("string contains reserved character");
            }
            i += 1;
        }
        Self(Cow::Borrowed(s))
    }
}

impl TryFrom<String> for UriSafeString {
    type Error = UriSafeError;

    /// Attempts to convert a String to a `UriSafeString`, validating that it
    /// doesn't contain any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use obscuri::UriSafeString;
    ///
    /// let safe = UriSafeString::try_from("hello_world".to_string());
    /// assert!(safe.is_ok());
    ///
    /// let unsafe_str = UriSafeString::try_from("{hello}".to_string());
    /// assert!(unsafe_str.is_err());
    /// ```
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(&s)
    }
}

impl<'a> TryFrom<&'a str> for UriSafeString {
    type Error = UriSafeError;

    /// Attempts to convert a `&str` to a `UriSafeString`, validating that it
    /// doesn't contain any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use obscuri::UriSafeString;
    ///
    /// let safe = UriSafeString::try_from("hello_world");
    /// assert!(safe.is_ok());
    ///
    /// let unsafe_str = UriSafeString::try_from("{hello}");
    /// assert!(unsafe_str.is_err());
    /// ```
    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        Self::new(&s)
    }
}

impl AsRef<str> for UriSafeString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for UriSafeString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESERVED_CHARACTERS: &str = "{}/:?#[]@!$&'()*+,;=";

    macro_rules! test_static_reserved_fail {
        ($(($index:ident, $char:expr)),* $(,)?) => {
            $(
                #[test]
                #[should_panic(expected = "string contains reserved character")]
                fn $index() {
                    let _ = UriSafeString::from_static(concat!("hello", $char, "world"));
                }
            )*
        };

    }

    #[test]
    fn test_uri_safe_string_creation() {
        let safe = UriSafeString::new(&"hello_world").unwrap();
        assert_eq!(safe.as_ref(), "hello_world");

        for reserved in RESERVED_CHARACTERS.chars() {
            let unsafe_str = UriSafeString::new(&format!("hello_{reserved})_world"));
            assert_eq!(
                unsafe_str.unwrap_err().to_string(),
                format!("invalid character '{reserved}' at position 6 for URI safe string")
            );
        }
    }

    #[test]
    fn test_uri_safe_string_from_static() {
        let safe = UriSafeString::from_static("hello_world");
        assert_eq!(safe.as_str(), "hello_world");
    }

    #[test]
    fn test_try_from_string_valid() {
        let result = UriSafeString::try_from("valid_string_123".to_string());
        assert_eq!(result.unwrap().as_str(), "valid_string_123");
    }

    #[test]
    fn test_try_from_string_invalid() {
        let result = UriSafeString::try_from("invalid{string}".to_string());
        assert!(result.is_err());
        result.unwrap_err();
    }

    #[test]
    fn test_try_from_str_valid() {
        let result = UriSafeString::try_from("valid_str_456");
        assert_eq!(result.unwrap().as_str(), "valid_str_456");
    }

    #[test]
    fn test_try_from_str_invalid() {
        let result = UriSafeString::try_from("path/with/slashes");
        assert!(result.is_err());
        result.unwrap_err();
    }

    // separate module to namespace generated tests and avoid conflicts
    mod from_static_reserved_characters {
        use super::*;

        test_static_reserved_fail! {
            (curly_left, "{"),
            (curly_right, "}"),
            (slash, "/"),
            (colon, ":"),
            (question_mark, "?"),
            (hash, "#"),
            (square_left, "["),
            (square_right, "]"),
            (at, "@"),
            (exclamation_mark, "!"),
            (dollar, "$"),
            (ampersand, "&"),
            (apostrophe, "'"),
            (parentheses_left, "("),
            (parentheses_right, ")"),
            (asterisk, "*"),
            (plus, "+"),
            (comma, ","),
            (semicolon, ";"),
            (equal, "=")
        }
    }
}
