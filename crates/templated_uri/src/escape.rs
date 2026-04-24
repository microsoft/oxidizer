// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize};

use data_privacy::Sensitive;
#[cfg(feature = "uuid")]
use uuid::Uuid;

use crate::{Escaped, EscapedString};

/// Marks types usable from templates, e.g., `/get/{foo}`.
pub trait Escape {
    /// Returns this value wrapped in [`Escaped`], proving it is properly escaped for URI use.
    fn escape(&self) -> Escaped<impl Display>;
}

/// Marks types with possibly dodgy content usable from templates, e.g., `/get/{+foo}`.
pub trait UnescapedDisplay {
    /// Returns a displayable representation of this value.
    fn unescaped_display(&self) -> impl Display;
}

macro_rules! impl_uri_unsafe_param {
    ($t:ty) => {
        impl UnescapedDisplay for $t {
            fn unescaped_display(&self) -> impl Display {
                self
            }
        }
    };
}

macro_rules! impl_uri_param {
    ($t:ty) => {
        impl Escape for $t {
            fn escape(&self) -> Escaped<impl Display> {
                Escaped::from(*self)
            }
        }
    };
}

impl_uri_unsafe_param!(String);

impl Escape for EscapedString {
    fn escape(&self) -> Escaped<impl Display> {
        self.clone()
    }
}

impl UnescapedDisplay for EscapedString {
    fn unescaped_display(&self) -> impl Display {
        self.as_str()
    }
}

impl_uri_param!(usize);
impl_uri_param!(u8);
impl_uri_param!(u16);
impl_uri_param!(u32);
impl_uri_param!(u64);
impl_uri_param!(u128);
impl_uri_param!(NonZeroU8);
impl_uri_param!(NonZeroU16);
impl_uri_param!(NonZeroU32);
impl_uri_param!(NonZeroU64);
impl_uri_param!(NonZeroU128);
impl_uri_param!(NonZeroUsize);
impl_uri_param!(IpAddr);
#[cfg(feature = "uuid")]
impl_uri_param!(Uuid);

impl<T> UnescapedDisplay for Sensitive<T>
where
    T: Display,
{
    fn unescaped_display(&self) -> impl Display {
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
    fn test_uri_unsafe_param_string() {
        let value = String::from("test_value");
        let display = value.unescaped_display();
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
    fn uri_unsafe_param_uri_escaped_string() {
        let s = EscapedString::escape("hello world");
        assert_eq!(format!("{}", s.unescaped_display()), "hello%20world");
    }

    #[test]
    fn test_uri_unsafe_param_sensitive() {
        // Test line 78-84: UnescapedDisplay for Sensitive<T> where T: Display
        let data_class = DataClass::new("test", "sensitive");
        let sensitive_string = Sensitive::new(String::from("secret_value"), data_class);

        let display = sensitive_string.unescaped_display();
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
