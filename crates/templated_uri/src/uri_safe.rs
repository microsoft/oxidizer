// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use pct_str::{PctString, UriReserved};
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
    /// Creates a `UriSafeString` by percent-encoding any reserved or unsafe characters.
    ///
    /// This is the preferred constructor - it always succeeds by encoding any characters
    /// that are not safe for URI templates as defined in RFC 6570.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriSafeString;
    ///
    /// let safe = UriSafeString::encode("hello_world");
    /// assert_eq!(safe.as_str(), "hello_world");
    ///
    /// let escaped_safe = UriSafeString::encode("{hello}");
    /// assert_eq!(escaped_safe.as_str(), "%7Bhello%7D");
    /// ```
    ///
    pub fn encode(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let encoded = PctString::encode(s.chars(), UriReserved::Any);
        // If nothing was encoded the PctString contains the same characters - avoid
        // a redundant allocation by comparing lengths (encoding can only grow the string).
        if encoded.as_str().len() == s.len() {
            Self(Cow::Owned(s.to_owned()))
        } else {
            Self(Cow::Owned(encoded.into_string()))
        }
    }

    /// Creates a `UriSafeString` from an already-encoded string, validating that it
    /// contains only characters that are safe for URI templates as defined in RFC 6570.
    ///
    /// Unlike [`UriSafeString::encode`], this constructor does **not** encode anything -
    /// it rejects the input if any reserved or unsafe character is found.
    /// Use this when you already have a percent-encoded string and want to enforce
    /// the invariant at the call site.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriSafeString;
    ///
    /// let safe = UriSafeString::try_new("hello_world");
    /// assert!(safe.is_ok());
    ///
    /// let unsafe_str = UriSafeString::try_new("{hello}");
    /// assert!(unsafe_str.is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`UriSafeError`] if the string contains reserved URI characters.
    pub fn try_new(raw: impl Into<String>) -> Result<Self, UriSafeError> {
        let raw = raw.into();

        let mut characters = raw.chars().enumerate();

        while let Some((i, c)) = characters.next() {
            if c == '%' {
                // Check URL encoded string - must have exactly 2 hex digits after %
                for _ in 0..2 {
                    if !characters.next().is_some_and(|(_, c)| c.is_ascii_hexdigit()) {
                        return Err(UriSafeError {
                            invalid_char: '%',
                            position: i,
                        });
                    }
                }
                // Valid percent-encoded sequence, continue to next character
                continue;
            }

            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '~' | '.') {
                continue;
            }

            return Err(UriSafeError {
                invalid_char: c,
                position: i,
            });
        }

        Ok(Self(Cow::Owned(raw)))
    }

    /// Returns a reference to the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow()
    }

    /// Creates a `UriSafeString` from a string literal, verifying at compile time
    /// that the string does not contain any reserved characters.
    ///
    /// Unlike [`UriSafeString::encode`], string needs to be percent-encoded beforehand
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriSafeString;
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
    #[inline]
    #[must_use]
    pub fn from_static(s: &'static str) -> Self {
        // Use the same validation logic as in uri_safe! macro
        let bytes = s.as_bytes();
        let mut i = 0;
        let mut url_encoded_char: Option<u8> = None;

        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            // We are dealing with URL encoded string
            if let Some(pct_num) = url_encoded_char {
                assert!(b.is_ascii_hexdigit(), "string contains invalid URL encoding character");

                // If we are at the second character already, disable URL encoded check and continue
                if pct_num == 1 {
                    url_encoded_char = None;
                    continue;
                }
                url_encoded_char = Some(pct_num + 1);
            }

            if b == b'%' {
                // URL encoded start
                url_encoded_char = Some(0);
                continue;
            }

            assert!(
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'~' | b'.'),
                "any reserved characters need to be URL encoded"
            );
        }
        assert!(url_encoded_char.is_none(), "string contains unfinished URL encoded character");
        Self(Cow::Borrowed(s))
    }
}

impl From<String> for UriSafeString {
    /// Converts a String to a `UriSafeString`, automatically percent-encoding
    /// any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriSafeString;
    ///
    /// let safe = UriSafeString::from("hello_world".to_string());
    /// assert_eq!(safe.as_str(), "hello_world");
    ///
    /// let encoded = UriSafeString::from("{hello}".to_string());
    /// assert_eq!(encoded.as_str(), "%7Bhello%7D");
    /// ```
    fn from(s: String) -> Self {
        Self::encode(s)
    }
}

impl<'a> From<&'a str> for UriSafeString {
    /// Converts a `&str` to a `UriSafeString`, automatically percent-encoding
    /// any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriSafeString;
    ///
    /// let safe = UriSafeString::from("hello_world");
    /// assert_eq!(safe.as_str(), "hello_world");
    ///
    /// let encoded = UriSafeString::from("{hello}");
    /// assert_eq!(encoded.as_str(), "%7Bhello%7D");
    /// ```
    fn from(s: &'a str) -> Self {
        Self::encode(s)
    }
}

impl AsRef<str> for UriSafeString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for UriSafeString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
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
                #[should_panic(expected = "any reserved characters need to be URL encoded")]
                fn $index() {
                    let _ = UriSafeString::from_static(concat!("hello", $char, "world"));
                }
            )*
        };

    }

    #[test]
    fn test_uri_safe_string_creation() {
        let safe = UriSafeString::encode("hello_world");
        assert_eq!(safe.as_ref(), "hello_world");

        for reserved in RESERVED_CHARACTERS.chars() {
            let encoded_str = UriSafeString::encode(format!("hello_{reserved}_world"));
            assert_eq!(encoded_str.to_string(), format!("hello_%{:02X}_world", reserved as u8));
        }
    }

    #[test]
    fn test_uri_safe_string_from_static() {
        let safe = UriSafeString::from_static("hello_world");
        assert_eq!(safe.as_str(), "hello_world");
    }

    #[test]
    fn test_from_string_valid() {
        let result = UriSafeString::from("valid_string_123".to_string());
        assert_eq!(result.as_str(), "valid_string_123");
    }

    #[test]
    fn test_raw_string_valid() {
        let result = UriSafeString::try_new("valid_string_123".to_string());
        assert_eq!(result.unwrap().as_str(), "valid_string_123");
    }

    #[test]
    fn try_new_accepts_valid_percent_encoded_sequence() {
        // A valid %XX sequence must be accepted - catches mutation that deletes `!`
        // in the is_some_and check, which would incorrectly reject valid encodings.
        let result = UriSafeString::try_new("hello%3Dworld");
        assert!(result.is_ok(), "valid percent-encoded sequence should be accepted");
        assert_eq!(result.unwrap().as_str(), "hello%3Dworld");
    }

    #[test]
    fn test_try_new_invalid_percent_encoding() {
        let result = UriSafeString::try_new("hello%3world".to_string());
        assert!(result.is_err(), "string with invalid percent encoding should be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.invalid_char, '%', "error should indicate the '%' character as invalid");
        assert_eq!(err.position, 5, "error should indicate the position of the invalid '%' character");
    }

    #[test]
    fn uri_safe_error_display_contains_char_and_position() {
        let err = UriSafeError {
            invalid_char: '{',
            position: 5,
        };
        let msg = err.to_string();
        assert!(msg.contains('{'), "error message should contain the invalid character");
        assert!(msg.contains('5'), "error message should contain the position");
    }

    #[test]
    fn test_from_string_reserved() {
        let result = UriSafeString::from("reserved{string}".to_string());
        assert_eq!(result.as_str(), "reserved%7Bstring%7D");
    }

    #[test]
    fn test_raw_string_reserved() {
        let result = UriSafeString::try_new("invalid{string}".to_string());
        assert!(result.is_err());
        result.unwrap_err();
    }

    #[test]
    fn test_from_str_valid() {
        let result = UriSafeString::from("valid_str_456");
        assert_eq!(result.as_str(), "valid_str_456");
    }

    #[test]
    fn test_from_str_reserved() {
        let result = UriSafeString::from("reserved{string}");
        assert_eq!(result.as_str(), "reserved%7Bstring%7D");
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

    #[test]
    fn from_static_urlencoded() {
        let result = UriSafeString::from_static("hello%3Dworld");
        assert_eq!(result.as_str(), "hello%3Dworld");
    }

    #[test]
    #[should_panic(expected = "string contains unfinished URL encoded character")]
    fn from_static_urlencoded_short() {
        let _ = UriSafeString::from_static("hello%3");
    }

    #[test]
    #[should_panic(expected = "string contains invalid URL encoding character")]
    fn from_static_urlencoded_bad_char() {
        let _ = UriSafeString::from_static("hello%3-world");
    }
}
