// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::Cow;
use alloc::string::String;
use core::str::FromStr;

use super::{Error, ErrorKind};

/// A decoded query component.
pub type Decoded<'q> = Cow<'q, str>;

/// Extracts a borrowed value without allocating.
///
/// # Errors
///
/// Returns [`ErrorKind::BorrowRequired`] when decoding allocated.
pub fn parse_borrowed<'q>(value: &Decoded<'q>, parameter: &'static str, pair_offset: usize) -> Result<&'q str, Error> {
    match value {
        Cow::Borrowed(value) => Ok(*value),
        Cow::Owned(_) => Err(Error::parsing(Some(parameter), pair_offset, ErrorKind::BorrowRequired)),
    }
}

/// Returns a decoded borrowed-or-owned value.
#[must_use]
pub fn parse_cow(value: Decoded<'_>) -> Decoded<'_> {
    value
}

/// Returns an owned decoded value.
#[must_use]
pub fn parse_owned(value: Decoded<'_>) -> String {
    value.into_owned()
}

/// Parses a decoded value through [`FromStr`].
///
/// # Errors
///
/// Returns [`ErrorKind::InvalidValue`] when parsing fails.
pub fn parse_value<T: FromStr>(value: &Decoded<'_>, parameter: &'static str, pair_offset: usize) -> Result<T, Error> {
    value
        .parse()
        .map_err(|_error| Error::parsing(Some(parameter), pair_offset, ErrorKind::InvalidValue))
}
