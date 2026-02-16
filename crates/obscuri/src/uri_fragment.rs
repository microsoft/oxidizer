// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize};

use data_privacy::Sensitive;
use uuid::Uuid;

use crate::{UriSafe, UriSafeString};

/// Marks types usable from templates, e.g., `/get/{foo}`.
pub trait UriFragment {
    /// Returns a reference to this value as a URI-safe type.
    fn as_uri_safe(&self) -> impl UriSafe;
}
//
/// Marks types with possibly dodgy content usable from templates, e.g., `/get/{foo}`
pub trait UriUnsafeFragment {
    /// Returns a displayable representation of this value.
    fn as_display(&self) -> impl Display;
}

macro_rules! impl_uri_unsafe_fragment {
    ($t:ty) => {
        impl UriUnsafeFragment for $t {
            fn as_display(&self) -> impl Display {
                self.to_string()
            }
        }
    };
}

macro_rules! impl_uri_fragment {
    ($t:ty) => {
        impl UriFragment for $t {
            fn as_uri_safe(&self) -> impl UriSafe {
                self
            }
        }
    };
}

impl_uri_unsafe_fragment!(String);
// TODO: This is more of a design choice if we want these or not
// impl_uri_unsafe_fragment!(usize);
// impl_uri_unsafe_fragment!(u8);
// impl_uri_unsafe_fragment!(u16);
// impl_uri_unsafe_fragment!(u32);
// impl_uri_unsafe_fragment!(u64);
// impl_uri_unsafe_fragment!(u128);
// impl_uri_unsafe_fragment!(NonZeroU8);
// impl_uri_unsafe_fragment!(NonZeroU16);
// impl_uri_unsafe_fragment!(NonZeroU32);
// impl_uri_unsafe_fragment!(NonZeroU64);
// impl_uri_unsafe_fragment!(NonZeroU128);
// impl_uri_unsafe_fragment!(NonZeroUsize);
// impl_uri_unsafe_fragment!(IpAddr);
// impl_uri_unsafe_fragment!(Uuid);

impl_uri_fragment!(UriSafeString);
impl_uri_fragment!(usize);
impl_uri_fragment!(u8);
impl_uri_fragment!(u16);
impl_uri_fragment!(u32);
impl_uri_fragment!(u64);
impl_uri_fragment!(u128);
impl_uri_fragment!(NonZeroU8);
impl_uri_fragment!(NonZeroU16);
impl_uri_fragment!(NonZeroU32);
impl_uri_fragment!(NonZeroU64);
impl_uri_fragment!(NonZeroU128);
impl_uri_fragment!(NonZeroUsize);
impl_uri_fragment!(IpAddr);
impl_uri_fragment!(Uuid);

impl<T> UriUnsafeFragment for Sensitive<T>
where
    T: Display,
{
    fn as_display(&self) -> impl Display {
        self.declassify_ref()
    }
}

impl<T> UriFragment for Sensitive<T>
where
    T: UriSafe,
{
    fn as_uri_safe(&self) -> impl UriSafe {
        self.declassify_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_unsafe_fragment_string() {
        let value = String::from("test_value");
        let display = value.as_display();
        assert_eq!(format!("{display}"), "test_value");
    }

    #[test]
    fn test_uri_fragment_unsigned_integer() {
        let value: u32 = 42;
        let uri_safe = value.as_uri_safe();
        assert_eq!(format!("{uri_safe}"), "42");
    }
}

