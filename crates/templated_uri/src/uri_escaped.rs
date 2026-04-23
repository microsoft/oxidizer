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

/// A wrapper that proves the inner value is already escaped for use in URI templates.
///
/// The invariant is enforced via constructors - only types whose [`Display`] output
/// contains no RFC 6570 reserved characters can be wrapped. For inherently-safe
/// types (integers, [`IpAddr`]) an infallible [`From`] impl is provided.
/// With the `uuid` feature (enabled by default), `Uuid` is also supported.
/// For strings, use the encoding/validating constructors on [`UriEscaped<Cow<'static, str>>`]
/// (aliased as [`UriEscapedString`]).
#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct UriEscaped<T>(T);

impl<T: Display> Display for UriEscaped<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Debug> Debug for UriEscaped<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

macro_rules! impl_uri_escaped_from {
    ($($t:ty),*) => {
        $(
            impl From<$t> for UriEscaped<$t> {
                fn from(value: $t) -> Self {
                    Self(value)
                }
            }
        )*
    };
}

impl_uri_escaped_from!(
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
impl_uri_escaped_from!(Uuid);

/// A URI-escaped string whose content is guaranteed to contain only characters
/// permitted in URI templates as defined by RFC 6570 (anything else is percent-encoded).
///
/// This is a type alias for `UriEscaped<Cow<'static, str>>`. Use its constructors
/// (`escape`, `try_new`, `from_static`) to create instances.
pub type UriEscapedString = UriEscaped<Cow<'static, str>>;

/// Error returned when a string is not a valid URI-escaped string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UriEscapeError(&'static str);

impl fmt::Display for UriEscapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for UriEscapeError {}

impl UriEscapedString {
    /// Creates a `UriEscapedString` by percent-encoding any reserved or invalid characters.
    ///
    /// This is the preferred constructor - it always succeeds by encoding any characters
    /// that are not permitted unescaped in URI templates as defined in RFC 6570.
    ///
    /// Accepts both borrowed (`&str`) and owned (`String`) inputs via `Into<Cow<'_, str>>`.
    /// When an owned `String` is provided and no encoding is needed, the original
    /// allocation is reused.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriEscapedString;
    ///
    /// // Borrowed input.
    /// let valid = UriEscapedString::escape("hello_world");
    /// assert_eq!(valid.as_str(), "hello_world");
    ///
    /// let escaped = UriEscapedString::escape("{hello}");
    /// assert_eq!(escaped.as_str(), "%7Bhello%7D");
    ///
    /// // Owned input; the String is reused directly when already valid.
    /// let valid = UriEscapedString::escape(String::from("hello_world"));
    /// assert_eq!(valid.as_str(), "hello_world");
    /// ```
    pub fn escape<'a>(s: impl Into<Cow<'a, str>>) -> Self {
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

    /// Creates a `UriEscapedString` from an already-encoded string, validating that it
    /// contains only characters that are permitted unescaped in URI templates as defined in RFC 6570.
    ///
    /// Unlike [`UriEscapedString::escape`], this constructor does **not** encode anything -
    /// it rejects the input if any reserved or invalid character is found.
    /// Use this when you already have a percent-encoded string and want to enforce
    /// the invariant at the call site.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriEscapedString;
    ///
    /// let valid = UriEscapedString::try_new("hello_world");
    /// assert!(valid.is_ok());
    ///
    /// let invalid = UriEscapedString::try_new("{hello}");
    /// assert!(invalid.is_err());
    /// ```
    ///
    /// Accepts both borrowed (`&'static str`) and owned (`String`) inputs via
    /// `Into<Cow<'static, str>>`. A `&'static str` is stored without allocation.
    ///
    /// # Errors
    ///
    /// Returns a [`UriEscapeError`] if the string contains reserved URI characters.
    pub fn try_new(raw: impl Into<Cow<'static, str>>) -> Result<Self, UriEscapeError> {
        Self::try_new_inner(raw.into())
    }

    fn try_new_inner(raw: Cow<'static, str>) -> Result<Self, UriEscapeError> {
        match validate_escaped(raw.as_bytes()) {
            None => Ok(Self(raw)),
            Some(message) => Err(UriEscapeError(message)),
        }
    }

    /// Returns a reference to the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.borrow()
    }

    /// Creates a `UriEscapedString` from a string literal.
    ///
    /// This is a `const fn`, so when used in a `const` context the validation runs at
    /// compile time. When called at runtime, invalid input panics instead.
    ///
    /// Unlike [`UriEscapedString::escape`], the input must already be percent-encoded -
    /// reserved characters are rejected rather than encoded.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriEscapedString;
    ///
    /// // Validated at compile time when used in a const context.
    /// const VALID: UriEscapedString = UriEscapedString::from_static("hello_world");
    ///
    /// // Also usable at runtime; panics on invalid input.
    /// let valid = UriEscapedString::from_static("hello_world");
    ///
    /// // The following would fail to compile (const) or panic at runtime:
    /// // const BAD: UriEscapedString = UriEscapedString::from_static("{hello}");
    /// ```
    ///
    /// # Panics
    /// if the provided string contains any reserved characters.
    #[cfg_attr(test, mutants::skip)] // Mutating this function leads to infinite loop and timeout
    #[inline]
    #[must_use]
    pub const fn from_static(s: &'static str) -> Self {
        match validate_escaped(s.as_bytes()) {
            None => Self(Cow::Borrowed(s)),
            Some(message) => panic!("{}", message),
        }
    }
}

/// Validates that `bytes` contains only RFC 6570 unreserved characters and well-formed
/// `%XX` percent-escape sequences. Returns `None` on success or a static description of
/// the first fault found. Shared by [`UriEscapedString::try_new`] (runtime) and
/// [`UriEscapedString::from_static`] (compile-time).
const fn validate_escaped(bytes: &[u8]) -> Option<&'static str> {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if i + 2 >= bytes.len() {
                return Some("string contains unfinished URL encoded character");
            }
            if !bytes[i + 1].is_ascii_hexdigit() || !bytes[i + 2].is_ascii_hexdigit() {
                return Some("string contains invalid URL encoding character");
            }
            i += 3;
            continue;
        }
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'~' | b'.') {
            i += 1;
            continue;
        }
        return Some("any reserved characters need to be URL encoded");
    }
    None
}

impl From<String> for UriEscapedString {
    /// Converts a String to a `UriEscapedString`, automatically percent-encoding
    /// any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriEscapedString;
    ///
    /// let valid = UriEscapedString::from("hello_world".to_string());
    /// assert_eq!(valid.as_str(), "hello_world");
    ///
    /// let encoded = UriEscapedString::from("{hello}".to_string());
    /// assert_eq!(encoded.as_str(), "%7Bhello%7D");
    /// ```
    fn from(s: String) -> Self {
        Self::escape(s)
    }
}

impl<'a> From<&'a str> for UriEscapedString {
    /// Converts a `&str` to a `UriEscapedString`, automatically percent-encoding
    /// any RFC 6570 reserved characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use templated_uri::UriEscapedString;
    ///
    /// let valid = UriEscapedString::from("hello_world");
    /// assert_eq!(valid.as_str(), "hello_world");
    ///
    /// let encoded = UriEscapedString::from("{hello}");
    /// assert_eq!(encoded.as_str(), "%7Bhello%7D");
    /// ```
    fn from(s: &'a str) -> Self {
        Self::escape(s)
    }
}

impl AsRef<str> for UriEscapedString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for UriEscapedString {
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
                    let _ = UriEscapedString::from_static(concat!("hello", $char, "world"));
                }
            )*
        };

    }

    #[test]
    fn test_uri_escaped_string_creation() {
        let safe = UriEscapedString::escape("hello_world");
        assert_eq!(safe.as_ref(), "hello_world");

        for reserved in RESERVED_CHARACTERS.chars() {
            let encoded_str = UriEscapedString::escape(format!("hello_{reserved}_world"));
            assert_eq!(encoded_str.to_string(), format!("hello_%{:02X}_world", reserved as u8));
        }
    }

    #[test]
    fn debug_delegates_to_inner() {
        let safe = UriEscapedString::escape("hello");
        assert_eq!(format!("{safe:?}"), format!("{:?}", "hello"));

        let safe_num = UriEscaped::from(42u32);
        assert_eq!(format!("{safe_num:?}"), "42");
    }

    #[test]
    fn test_uri_escaped_string_from_static() {
        const SAFE: UriEscapedString = UriEscapedString::from_static("hello_world");
        assert_eq!(SAFE.as_str(), "hello_world");
    }

    #[test]
    fn test_from_string_valid() {
        let result = UriEscapedString::from("valid_string_123".to_string());
        assert_eq!(result.as_str(), "valid_string_123");
    }

    #[test]
    fn escape_owned_no_encoding_reuses_string() {
        // When given an owned `String` that requires no encoding, the original
        // allocation must be reused rather than copied.
        let input = "hello_world".to_string();
        let ptr_before = input.as_ptr();
        let valid = UriEscapedString::escape(input);
        assert_eq!(valid.as_str(), "hello_world");
        assert_eq!(valid.as_str().as_ptr(), ptr_before);
    }

    #[test]
    fn escape_owned_encodes_reserved() {
        let valid = UriEscapedString::escape("hello{world}".to_string());
        assert_eq!(valid.as_str(), "hello%7Bworld%7D");
    }

    #[test]
    fn test_raw_string_valid() {
        let result = UriEscapedString::try_new("valid_string_123".to_string());
        assert_eq!(result.unwrap().as_str(), "valid_string_123");
    }

    #[test]
    fn try_new_accepts_valid_percent_encoded_sequence() {
        // A valid %XX sequence must be accepted - catches mutation that deletes `!`
        // in the is_some_and check, which would incorrectly reject valid encodings.
        let result = UriEscapedString::try_new("hello%3Dworld");
        assert!(result.is_ok(), "valid percent-encoded sequence should be accepted");
        assert_eq!(result.unwrap().as_str(), "hello%3Dworld");
    }

    #[test]
    fn try_new_preserves_static_borrow() {
        // A `&'static str` input must be stored without copying.
        const INPUT: &str = "hello_world";
        let result = UriEscapedString::try_new(INPUT).unwrap();
        assert_eq!(result.as_str().as_ptr(), INPUT.as_ptr());
    }

    #[test]
    fn try_new_preserves_owned_string() {
        // An owned `String` input must be reused without copying.
        let input = "hello_world".to_string();
        let ptr_before = input.as_ptr();
        let result = UriEscapedString::try_new(input).unwrap();
        assert_eq!(result.as_str().as_ptr(), ptr_before);
    }

    #[test]
    fn test_try_new_invalid_percent_encoding() {
        let result = UriEscapedString::try_new("hello%3world".to_string());
        let err = result.unwrap_err();
        assert_eq!(err.to_string(), "string contains invalid URL encoding character");
    }

    #[test]
    fn uri_escape_error_display_matches_message() {
        let err = UriEscapedString::try_new("hello{world}".to_string()).unwrap_err();
        assert_eq!(err.to_string(), "any reserved characters need to be URL encoded");
    }

    #[test]
    fn test_from_string_reserved() {
        let result = UriEscapedString::from("reserved{string}".to_string());
        assert_eq!(result.as_str(), "reserved%7Bstring%7D");
    }

    #[test]
    fn test_raw_string_reserved() {
        let result = UriEscapedString::try_new("invalid{string}".to_string());
        assert!(result.is_err());
        result.unwrap_err();
    }

    #[test]
    fn test_from_str_valid() {
        let result = UriEscapedString::from("valid_str_456");
        assert_eq!(result.as_str(), "valid_str_456");
    }

    #[test]
    fn test_from_str_reserved() {
        let result = UriEscapedString::from("reserved{string}");
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
        let result = UriEscapedString::from_static("hello%3Dworld");
        assert_eq!(result.as_str(), "hello%3Dworld");
    }

    #[test]
    #[should_panic(expected = "string contains unfinished URL encoded character")]
    fn from_static_urlencoded_short() {
        let _ = UriEscapedString::from_static("hello%3");
    }

    #[test]
    #[should_panic(expected = "string contains invalid URL encoding character")]
    fn from_static_urlencoded_bad_char() {
        let _ = UriEscapedString::from_static("hello%3-world");
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn uri_escaped_string_roundtrip() {
            let original = UriEscapedString::escape("hello world");
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, r#""hello%20world""#);
            let deserialized: UriEscapedString = serde_json::from_str(&json).unwrap();
            assert_eq!(original, deserialized);
        }

        #[test]
        fn uri_escaped_string_deserialize_rejects_reserved() {
            serde_json::from_str::<UriEscapedString>(r#""hello{world}""#).unwrap_err();
        }
    }
}
