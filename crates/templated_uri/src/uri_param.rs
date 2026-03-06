// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize};

use data_privacy::Sensitive;
#[cfg(feature = "uuid")]
use uuid::Uuid;

use crate::{UriSafe, UriSafeString};

/// Marks types usable from templates, e.g., `/get/{foo}`.
pub trait UriParam {
    /// Returns this value wrapped in [`UriSafe`], proving it is safe for URI use.
    fn as_uri_safe(&self) -> UriSafe<impl Display>;
}
//
/// Marks types with possibly dodgy content usable from templates, e.g., `/get/{foo}`
pub trait UriUnsafeParam {
    /// Returns a displayable representation of this value.
    fn as_display(&self) -> impl Display;
}

macro_rules! impl_uri_unsafe_param {
    ($t:ty) => {
        impl UriUnsafeParam for $t {
            fn as_display(&self) -> impl Display {
                self
            }
        }
    };
}

macro_rules! impl_uri_param {
    ($t:ty) => {
        impl UriParam for $t {
            fn as_uri_safe(&self) -> UriSafe<impl Display> {
                UriSafe::from(*self)
            }
        }
    };
}

impl_uri_unsafe_param!(String);

impl UriParam for UriSafeString {
    fn as_uri_safe(&self) -> UriSafe<impl Display> {
        self.clone()
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

impl<T> UriUnsafeParam for Sensitive<T>
where
    T: Display,
{
    fn as_display(&self) -> impl Display {
        self.declassify_ref()
    }
}

impl<T> UriParam for Sensitive<T>
where
    T: UriParam,
{
    fn as_uri_safe(&self) -> UriSafe<impl Display> {
        self.declassify_ref().as_uri_safe()
    }
}

#[cfg(test)]
mod tests {
    use data_privacy::DataClass;

    use super::*;

    #[test]
    fn test_uri_unsafe_param_string() {
        let value = String::from("test_value");
        let display = value.as_display();
        assert_eq!(format!("{display}"), "test_value");
    }

    #[test]
    fn test_uri_param_unsigned_integer() {
        let value: u32 = 42;
        let uri_safe = value.as_uri_safe();
        assert_eq!(format!("{uri_safe}"), "42");
    }

    #[test]
    fn test_uri_unsafe_param_sensitive() {
        // Test line 78-84: UriUnsafeParam for Sensitive<T> where T: Display
        let data_class = DataClass::new("test", "sensitive");
        let sensitive_string = Sensitive::new(String::from("secret_value"), data_class);

        let display = sensitive_string.as_display();
        assert_eq!(format!("{display}"), "secret_value");
    }

    #[test]
    fn test_uri_param_sensitive() {
        // Test UriParam for Sensitive<T> where T: UriParam
        let data_class = DataClass::new("test", "safe");
        let sensitive_num = Sensitive::new(100u32, data_class);

        let uri_safe = sensitive_num.as_uri_safe();
        assert_eq!(format!("{uri_safe}"), "100");
    }
}
