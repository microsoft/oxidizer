// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize};

use data_privacy::Sensitive;
#[cfg(feature = "uuid")]
use uuid::Uuid;

use crate::{Escaped, EscapedString};

/// Marks types whose values are percent-encoded before being inserted into a URI.
///
/// Used for RFC 6570 simple-expansion placeholders (`{foo}`), where reserved characters
/// like `/`, `?`, and `#` must be percent-encoded so the resulting URI parses as intended.
/// For reserved-expansion placeholders (`{+foo}`) that emit reserved characters verbatim,
/// use [`Raw`] instead.
///
/// The returned [`Escaped`] wrapper acts as a proof-token that the value is safe to splice
/// into a URI without further encoding.
pub trait Escape {
    /// Returns this value wrapped in [`Escaped`], proving it is properly escaped for URI use.
    fn escape(&self) -> Escaped<impl Display>;
}

/// Marks types whose `Display` output is emitted verbatim into a URI, without
/// percent-encoding reserved characters.
///
/// Used for RFC 6570 reserved expansion placeholders (`{+foo}`), where characters
/// like `/`, `?`, and `#` are intentionally allowed through unchanged. For ordinary
/// placeholders (`{foo}`) that must percent-encode reserved characters, use
/// [`Escape`] instead.
///
/// Implementers are responsible for ensuring the rendered output is valid in the
/// target URI position; no encoding is performed by the render.
pub trait Raw {
    /// Returns this value's raw `Display` form, to be inserted into a URI without escaping.
    fn raw(&self) -> impl Display;
}

macro_rules! impl_raw {
    ($t:ty) => {
        impl Raw for $t {
            fn raw(&self) -> impl Display {
                self
            }
        }
    };
}

macro_rules! impl_escape {
    ($t:ty) => {
        impl Escape for $t {
            fn escape(&self) -> Escaped<impl Display> {
                Escaped::from(*self)
            }
        }
    };
}

impl_raw!(String);

impl Escape for EscapedString {
    fn escape(&self) -> Escaped<impl Display> {
        self.clone()
    }
}

impl Raw for EscapedString {
    fn raw(&self) -> impl Display {
        self.as_str()
    }
}

impl_escape!(usize);
impl_escape!(u8);
impl_escape!(u16);
impl_escape!(u32);
impl_escape!(u64);
impl_escape!(u128);
impl_escape!(NonZeroU8);
impl_escape!(NonZeroU16);
impl_escape!(NonZeroU32);
impl_escape!(NonZeroU64);
impl_escape!(NonZeroU128);
impl_escape!(NonZeroUsize);
impl_escape!(IpAddr);
#[cfg(feature = "uuid")]
impl_escape!(Uuid);

impl<T> Raw for Sensitive<T>
where
    T: Display,
{
    fn raw(&self) -> impl Display {
        self.declassify_ref()
    }
}

impl<T> Escape for Sensitive<T>
where
    T: Escape,
{
    fn escape(&self) -> Escaped<impl Display> {
        self.declassify_ref().escape()
    }
}

#[cfg(test)]
mod tests {
    use data_privacy::DataClass;

    use super::*;

    #[test]
    fn test_raw_string() {
        let value = String::from("test_value");
        let display = value.raw();
        assert_eq!(format!("{display}"), "test_value");
    }

    #[test]
    fn test_uri_param_unsigned_integer() {
        let value: u32 = 42;
        let uri_safe = value.escape();
        assert_eq!(format!("{uri_safe}"), "42");
    }

    #[test]
    fn uri_param_all_numeric_types() {
        assert_eq!(format!("{}", 1u8.escape()), "1");
        assert_eq!(format!("{}", 2u16.escape()), "2");
        assert_eq!(format!("{}", 3u64.escape()), "3");
        assert_eq!(format!("{}", 4u128.escape()), "4");
        assert_eq!(format!("{}", 5usize.escape()), "5");
        assert_eq!(format!("{}", NonZeroU8::new(1).unwrap().escape()), "1");
        assert_eq!(format!("{}", NonZeroU16::new(2).unwrap().escape()), "2");
        assert_eq!(format!("{}", NonZeroU32::new(3).unwrap().escape()), "3");
        assert_eq!(format!("{}", NonZeroU64::new(4).unwrap().escape()), "4");
        assert_eq!(format!("{}", NonZeroU128::new(5).unwrap().escape()), "5");
        assert_eq!(format!("{}", NonZeroUsize::new(6).unwrap().escape()), "6");
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(format!("{}", ip.escape()), "127.0.0.1");
    }

    #[test]
    fn uri_param_uri_escaped_string() {
        let s = EscapedString::escape("hello");
        assert_eq!(format!("{}", s.escape()), "hello");
    }

    #[test]
    fn raw_uri_escaped_string() {
        let s = EscapedString::escape("hello world");
        assert_eq!(format!("{}", s.raw()), "hello%20world");
    }

    #[test]
    fn test_raw_sensitive() {
        // Test Raw for Sensitive<T> where T: Display
        let data_class = DataClass::new("test", "sensitive");
        let sensitive_string = Sensitive::new(String::from("secret_value"), data_class);

        let display = sensitive_string.raw();
        assert_eq!(format!("{display}"), "secret_value");
    }

    #[test]
    fn test_uri_param_sensitive() {
        // Test Escape for Sensitive<T> where T: Escape
        let data_class = DataClass::new("test", "safe");
        let sensitive_num = Sensitive::new(100u32, data_class);

        let uri_safe = sensitive_num.escape();
        assert_eq!(format!("{uri_safe}"), "100");
    }
}
