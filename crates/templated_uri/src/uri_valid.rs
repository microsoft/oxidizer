// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::{Borrow, Cow};
use std::error::Error;
use std::fmt;
use std::fmt::{Debug, Display};
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize};

use pct_str::{PctString, UriReserved};
#[cfg(feature = "uuid")]
use uuid::Uuid;

/// A wrapper that proves the inner value is valid for use in URI templates.
///
/// Validity is enforced via constructors - only types whose [`Display`] output
/// contains no RFC 6570 reserved characters can be wrapped. For inherently-valid
/// types (integers, [`IpAddr`]) an infallible [`From`] impl is provided.
/// With the `uuid` feature (enabled by default), `Uuid` is also supported.
/// For strings, use the encoding/validating constructors on [`UriValid<Cow<'static, str>>`]
/// (aliased as [`UriValidString`]).
#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct UriValid<T>(T);

impl<T: Display> Display for UriValid<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Debug> Debug for UriValid<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

macro_rules! impl_uri_valid_from {
    ($($t:ty),*) => {
        $(
            impl From<$t> for UriValid<$t> {
                fn from(value: $t) -> Self {
                    Self(value)
                }
            }
        )*
    };
}

impl_uri_valid_from!(
    usize,
    u8,
    u16,
    u32,
    u64,
    u128,
    NonZeroU8,
    NonZeroU16,
    NonZeroU32,
    NonZeroU64,
    NonZeroU128,
    NonZeroUsize,
    IpAddr
);

#[cfg(feature = "uuid")]
impl_uri_valid_from!(Uuid);

/// A URI-valid string whose content is guaranteed to contain only characters
/// valid in URI templates as defined by RFC 6570.
///
/// This is a type alias for `UriValid<Cow<'static, str>>`. Use its constructors
/// (`encode`, `try_new`, `from_static`) to create instances.
pub type UriValidString = UriValid<Cow<'static, str>>;

/// Error returned when a string contains characters that are not valid for URI templates.
#[derive(Debug)]
pub struct UriValidationError {
    /// The invalid character that was found.
    pub invalid_char: char,
    /// The position in the string where the invalid character was found.
    pub position: usize,
}

impl fmt::Display for UriValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid character '{}' at position {} for URI valid string",
            self.invalid_char, self.position
        )
    }
}

impl Error for UriValidationError {}

impl UriValidString {
    /// Creates a `UriValidString` by percent-encoding any reserved or invalid characters.
    ///
    /// This is the preferred constructor - it always succeeds by encoding any characters
    /// that are not valid for URI templates as defined in RFC 6570.
    ///
    /// Accepts both borrowed (`&str`) and owned (`String`) inputs via `Into<Cow<'_, str>>`.
    /// When an owned `String` is provided and no encoding is needed, the original
    /// allocation is reused.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriValidString;
    ///
    /// // Borrowed input.
    /// let valid = UriValidString::encode("hello_world");
    /// assert_eq!(valid.as_str(), "hello_world");
    ///
    /// let escaped = UriValidString::encode("{hello}");
    /// assert_eq!(escaped.as_str(), "%7Bhello%7D");
    ///
    /// // Owned input; the String is reused directly when already valid.
    /// let valid = UriValidString::encode(String::from("hello_world"));
    /// assert_eq!(valid.as_str(), "hello_world");
    /// ```
    pub fn encode<'a>(s: impl Into<Cow<'a, str>>) -> Self {
        let s = s.into();
        let encoded = PctString::encode(s.chars(), UriReserved::Any);
        if encoded.as_str().len() == s.len() {
            // No characters required encoding; avoid allocating a new `String` when
            // the caller already owns one.
            match s {
                Cow::Owned(owned) => Self(Cow::Owned(owned)),
                Cow::Borrowed(borrowed) => Self(Cow::Owned(borrowed.to_owned())),
            }
        } else {
            Self(Cow::Owned(encoded.into_string()))
        }
    }

    /// Creates a `UriValidString` from an already-encoded string, validating that it
    /// contains only characters that are valid for URI templates as defined in RFC 6570.
    ///
    /// Unlike [`UriValidString::encode`], this constructor does **not** encode anything -
    /// it rejects the input if any reserved or invalid character is found.
    /// Use this when you already have a percent-encoded string and want to enforce
    /// the invariant at the call site.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriValidString;
    ///
    /// let valid = UriValidString::try_new("hello_world");
    /// assert!(valid.is_ok());
    ///
    /// let invalid = UriValidString::try_new("{hello}");
    /// assert!(invalid.is_err());
    /// ```
    ///
    /// Accepts both borrowed (`&'static str`) and owned (`String`) inputs via
    /// `Into<Cow<'static, str>>`. A `&'static str` is stored without allocation.
    ///
    /// # Errors
    ///
    /// Returns a [`UriValidationError`] if the string contains reserved URI characters.
    pub fn try_new(raw: impl Into<Cow<'static, str>>) -> Result<Self, UriValidationError> {
        Self::try_new_inner(raw.into())
    }

    fn try_new_inner(raw: Cow<'static, str>) -> Result<Self, UriValidationError> {
        let mut characters = raw.chars().enumerate();

        while let Some((i, c)) = characters.next() {
            if c == '%' {
                // Check URL encoded string - must have exactly 2 hex digits after %
                for _ in 0..2 {
                    if !characters.next().is_some_and(|(_, c)| c.is_ascii_hexdigit()) {
                        return Err(UriValidationError {
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

            return Err(UriValidationError {
                invalid_char: c,
                position: i,
            });
        }

        Ok(Self(raw))
    }

    /// Returns a reference to the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow()
    }

    /// Creates a `UriValidString` from a string literal.
    ///
    /// This is a `const fn`, so when used in a `const` context the validation runs at
    /// compile time. When called at runtime, invalid input panics instead.
    ///
    /// Unlike [`UriValidString::encode`], the input must already be percent-encoded -
    /// reserved characters are rejected rather than encoded.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriValidString;
    ///
    /// // Validated at compile time when used in a const context.
    /// const VALID: UriValidString = UriValidString::from_static("hello_world");
    ///
    /// // Also usable at runtime; panics on invalid input.
    /// let valid = UriValidString::from_static("hello_world");
    ///
    /// // The following would fail to compile (const) or panic at runtime:
    /// // const BAD: UriValidString = UriValidString::from_static("{hello}");
    /// ```
    ///
    /// # Panics
    /// if the provided string contains any reserved characters.
    #[cfg_attr(test, mutants::skip)] // Mutating this function leads to infinite loop and timeout
    #[inline]
    #[must_use]
    pub const fn from_static(s: &'static str) -> Self {
        // Use the same validation logic as in uri_valid! macro
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

impl From<String> for UriValidString {
    /// Converts a String to a `UriValidString`, automatically percent-encoding
    /// any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriValidString;
    ///
    /// let valid = UriValidString::from("hello_world".to_string());
    /// assert_eq!(valid.as_str(), "hello_world");
    ///
    /// let encoded = UriValidString::from("{hello}".to_string());
    /// assert_eq!(encoded.as_str(), "%7Bhello%7D");
    /// ```
    fn from(s: String) -> Self {
        Self::encode(s)
    }
}

impl<'a> From<&'a str> for UriValidString {
    /// Converts a `&str` to a `UriValidString`, automatically percent-encoding
    /// any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriValidString;
    ///
    /// let valid = UriValidString::from("hello_world");
    /// assert_eq!(valid.as_str(), "hello_world");
    ///
    /// let encoded = UriValidString::from("{hello}");
    /// assert_eq!(encoded.as_str(), "%7Bhello%7D");
    /// ```
    fn from(s: &'a str) -> Self {
        Self::encode(s)
    }
}

impl AsRef<str> for UriValidString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for UriValidString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_new(s).map_err(serde::de::Error::custom)
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
                    let _ = UriValidString::from_static(concat!("hello", $char, "world"));
                }
            )*
        };

    }

    #[test]
    fn test_uri_valid_string_creation() {
        let safe = UriValidString::encode("hello_world");
        assert_eq!(safe.as_ref(), "hello_world");

        for reserved in RESERVED_CHARACTERS.chars() {
            let encoded_str = UriValidString::encode(format!("hello_{reserved}_world"));
            assert_eq!(encoded_str.to_string(), format!("hello_%{:02X}_world", reserved as u8));
        }
    }

    #[test]
    fn debug_delegates_to_inner() {
        let safe = UriValidString::encode("hello");
        assert_eq!(format!("{safe:?}"), format!("{:?}", "hello"));

        let safe_num = UriValid::from(42u32);
        assert_eq!(format!("{safe_num:?}"), "42");
    }

    #[test]
    fn test_uri_valid_string_from_static() {
        const SAFE: UriValidString = UriValidString::from_static("hello_world");
        assert_eq!(SAFE.as_str(), "hello_world");
    }

    #[test]
    fn test_from_string_valid() {
        let result = UriValidString::from("valid_string_123".to_string());
        assert_eq!(result.as_str(), "valid_string_123");
    }

    #[test]
    fn encode_owned_no_encoding_reuses_string() {
        // When given an owned `String` that requires no encoding, the original
        // allocation must be reused rather than copied.
        let input = "hello_world".to_string();
        let ptr_before = input.as_ptr();
        let valid = UriValidString::encode(input);
        assert_eq!(valid.as_str(), "hello_world");
        assert_eq!(valid.as_str().as_ptr(), ptr_before);
    }

    #[test]
    fn encode_owned_encodes_reserved() {
        let valid = UriValidString::encode("hello{world}".to_string());
        assert_eq!(valid.as_str(), "hello%7Bworld%7D");
    }

    #[test]
    fn test_raw_string_valid() {
        let result = UriValidString::try_new("valid_string_123".to_string());
        assert_eq!(result.unwrap().as_str(), "valid_string_123");
    }

    #[test]
    fn try_new_accepts_valid_percent_encoded_sequence() {
        // A valid %XX sequence must be accepted - catches mutation that deletes `!`
        // in the is_some_and check, which would incorrectly reject valid encodings.
        let result = UriValidString::try_new("hello%3Dworld");
        assert!(result.is_ok(), "valid percent-encoded sequence should be accepted");
        assert_eq!(result.unwrap().as_str(), "hello%3Dworld");
    }

    #[test]
    fn try_new_preserves_static_borrow() {
        // A `&'static str` input must be stored without copying.
        const INPUT: &str = "hello_world";
        let result = UriValidString::try_new(INPUT).unwrap();
        assert_eq!(result.as_str().as_ptr(), INPUT.as_ptr());
    }

    #[test]
    fn try_new_preserves_owned_string() {
        // An owned `String` input must be reused without copying.
        let input = "hello_world".to_string();
        let ptr_before = input.as_ptr();
        let result = UriValidString::try_new(input).unwrap();
        assert_eq!(result.as_str().as_ptr(), ptr_before);
    }

    #[test]
    fn test_try_new_invalid_percent_encoding() {
        let result = UriValidString::try_new("hello%3world".to_string());
        assert!(result.is_err(), "string with invalid percent encoding should be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.invalid_char, '%', "error should indicate the '%' character as invalid");
        assert_eq!(err.position, 5, "error should indicate the position of the invalid '%' character");
    }

    #[test]
    fn uri_validation_error_display_contains_char_and_position() {
        let err = UriValidationError {
            invalid_char: '{',
            position: 5,
        };
        let msg = err.to_string();
        assert!(msg.contains('{'), "error message should contain the invalid character");
        assert!(msg.contains('5'), "error message should contain the position");
    }

    #[test]
    fn test_from_string_reserved() {
        let result = UriValidString::from("reserved{string}".to_string());
        assert_eq!(result.as_str(), "reserved%7Bstring%7D");
    }

    #[test]
    fn test_raw_string_reserved() {
        let result = UriValidString::try_new("invalid{string}".to_string());
        assert!(result.is_err());
        result.unwrap_err();
    }

    #[test]
    fn test_from_str_valid() {
        let result = UriValidString::from("valid_str_456");
        assert_eq!(result.as_str(), "valid_str_456");
    }

    #[test]
    fn test_from_str_reserved() {
        let result = UriValidString::from("reserved{string}");
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
        let result = UriValidString::from_static("hello%3Dworld");
        assert_eq!(result.as_str(), "hello%3Dworld");
    }

    #[test]
    #[should_panic(expected = "string contains unfinished URL encoded character")]
    fn from_static_urlencoded_short() {
        let _ = UriValidString::from_static("hello%3");
    }

    #[test]
    #[should_panic(expected = "string contains invalid URL encoding character")]
    fn from_static_urlencoded_bad_char() {
        let _ = UriValidString::from_static("hello%3-world");
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn uri_valid_string_roundtrip() {
            let original = UriValidString::encode("hello world");
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, r#""hello%20world""#);
            let deserialized: UriValidString = serde_json::from_str(&json).unwrap();
            assert_eq!(original, deserialized);
        }

        #[test]
        fn uri_valid_string_deserialize_rejects_reserved() {
            serde_json::from_str::<UriValidString>(r#""hello{world}""#).unwrap_err();
        }
    }
}
