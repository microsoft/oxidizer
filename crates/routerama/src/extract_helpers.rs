// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field-coercion helpers invoked by `#[resolver]`-generated code.
//!
//! Static helpers coerce raw captures; dynamic helpers retrieve captures by
//! index before coercion.

use alloc::borrow::Cow;
use alloc::string::String;
use core::str::FromStr;

use crate::ResolveError;
use crate::captures::Captures;
use crate::decode::decode;

// Static route coercion.

/// `String` field of a static route: percent-decoded, owned.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8.
#[inline]
pub fn coerce_owned<'p>(raw: &'p str, field: &'static str) -> Result<String, ResolveError<'p>> {
    decode(raw).map(Cow::into_owned).ok_or(ResolveError::UndecodableCapture(field))
}

/// `Cow<str>` field of a static route: borrowed when no decoding is needed.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8.
#[inline]
pub fn coerce_cow<'p>(raw: &'p str, field: &'static str) -> Result<Cow<'p, str>, ResolveError<'p>> {
    decode(raw).ok_or(ResolveError::UndecodableCapture(field))
}

/// `T: FromStr` field of a static route: percent-decoded, then parsed.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8;
/// [`ResolveError::InvalidCapture`] when parsing fails.
pub fn coerce_parse<'p, T: FromStr>(raw: &'p str, field: &'static str) -> Result<T, ResolveError<'p>> {
    let decoded = decode(raw).ok_or(ResolveError::UndecodableCapture(field))?;
    decoded.parse::<T>().map_err(|_err| ResolveError::InvalidCapture(field))
}

// Dynamic route coercion.

/// `String` field of a dynamic route: percent-decoded, owned.
///
/// # Errors
/// [`ResolveError::MissingCapture`] when absent; [`ResolveError::UndecodableCapture`] on a malformed
/// escape or invalid UTF-8.
#[inline]
pub fn owned(captures: &Captures<'_, '_, '_>, index: usize, field: &'static str) -> Result<String, ResolveError<'static>> {
    let raw = captures.get(index).ok_or(ResolveError::MissingCapture(field))?;
    decode(raw).map(Cow::into_owned).ok_or(ResolveError::UndecodableCapture(field))
}

/// `T: FromStr` field of a dynamic route: percent-decoded, then parsed.
///
/// # Errors
/// [`ResolveError::MissingCapture`] when absent; [`ResolveError::UndecodableCapture`] on a malformed
/// escape or invalid UTF-8; [`ResolveError::InvalidCapture`] when parsing fails.
pub fn parse<T: FromStr>(captures: &Captures<'_, '_, '_>, index: usize, field: &'static str) -> Result<T, ResolveError<'static>> {
    let raw = captures.get(index).ok_or(ResolveError::MissingCapture(field))?;
    let decoded = decode(raw).ok_or(ResolveError::UndecodableCapture(field))?;
    decoded.parse::<T>().map_err(|_err| ResolveError::InvalidCapture(field))
}
