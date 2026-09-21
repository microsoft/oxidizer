// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-family blanket trait implementations and small parsing helpers
//! reused by more than one header family.

#![allow(
    dead_code,
    unused_imports,
    unused_macros,
    reason = "shared helpers are selected by independent header-family features"
)]

use std::{fmt, mem, slice, str};

use super::*;
use crate::sink::EncodedValues;
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef};

#[derive(Clone, Eq, Hash, PartialEq)]
pub(super) enum FieldLinesIter {
    Empty,
    One(FieldValue),
    Many(Vec<FieldValue>),
}

impl FieldLinesIter {
    pub(super) const fn empty() -> Self {
        Self::Empty
    }

    pub(super) const fn one(value: FieldValue) -> Self {
        Self::One(value)
    }

    pub(super) fn push(&mut self, value: FieldValue) {
        match self {
            Self::Empty => *self = Self::One(value),
            Self::One(first) => {
                let first = mem::replace(first, FieldValue::from_static(""));
                *self = Self::Many(vec![first, value]);
            }
            Self::Many(values) => values.push(value),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub(super) fn iter(&self) -> slice::Iter<'_, FieldValue> {
        self.as_slice().iter()
    }

    pub(super) fn iter_mut(&mut self) -> slice::IterMut<'_, FieldValue> {
        match self {
            Self::Empty => [].iter_mut(),
            Self::One(value) => slice::from_mut(value).iter_mut(),
            Self::Many(values) => values.iter_mut(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    pub(super) fn into_encoded(self) -> EncodedValues {
        match self {
            Self::Empty => EncodedValues::new(),
            Self::One(value) => EncodedValues::single(value),
            Self::Many(values) => EncodedValues::from_vec(values),
        }
    }

    pub(super) fn into_vec(self) -> Vec<FieldValue> {
        match self {
            Self::Empty => Vec::new(),
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }

    fn as_slice(&self) -> &[FieldValue] {
        match self {
            Self::Empty => &[],
            Self::One(value) => slice::from_ref(value),
            Self::Many(values) => values,
        }
    }
}

macro_rules! impl_from_str {
    ($($(#[$meta:meta])* ($owned:ty, $descriptor:ty)),+ $(,)?) => {
        $(
            $(#[$meta])*
            impl std::str::FromStr for $owned {
                type Err = DecodeError;

                #[inline(always)]
                fn from_str(value: &str) -> Result<Self, Self::Err> {
                    parse_field_value(<$descriptor as crate::Field>::name(), value)
                        .and_then(Self::try_from)
                }
            }
        )+
    };
}

macro_rules! impl_ascii_display {
    ($($(#[$meta:meta])* ($owned:ty, $descriptor:ty)),+ $(,)?) => {
        $(
            $(#[$meta])*
            impl std::fmt::Display for $owned {
                #[inline(always)]
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    let value = <$descriptor as crate::SingleValueField>::as_field_value(self);
                    fmt_ascii_value(value, f)
                }
            }
        )+
    };
}

macro_rules! impl_field_value_conversion {
    ($owned:ty, |$value:ident| $field_value:expr) => {
        impl From<$owned> for $crate::FieldValue {
            #[inline]
            fn from($value: $owned) -> Self {
                $field_value
            }
        }
    };
}

pub(super) use impl_field_value_conversion;

macro_rules! impl_string_conversions {
    ($owned:ty, $name:expr, $invalid:path, $input:ident) => {
        impl TryFrom<&str> for $owned {
            type Error = $crate::DecodeError;

            fn try_from($input: &str) -> Result<Self, Self::Error> {
                let value = $crate::FieldValue::from_str($input).map_err(|_invalid| $invalid($name))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<String> for $owned {
            type Error = $crate::DecodeError;

            fn try_from($input: String) -> Result<Self, Self::Error> {
                let value = $crate::FieldValue::try_from($input).map_err(|_invalid| $invalid($name))?;
                Self::try_from(value)
            }
        }
    };
}

pub(super) use impl_string_conversions;

macro_rules! impl_value_count_debug {
    ($($value:ty => $name:literal),+ $(,)?) => {
        $(
            impl std::fmt::Debug for $value {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.debug_struct($name)
                        .field("value_count", &self.values.len())
                        .finish()
                }
            }
        )+
    };
}

pub(super) use impl_value_count_debug;

fn parse_field_value(name: &'static FieldName, value: &str) -> Result<FieldValue, DecodeError> {
    match FieldValue::from_str(value) {
        Ok(value) => Ok(value),
        Err(_invalid) => Err(invalid_syntax(name)),
    }
}

fn ascii_value(value: &FieldValue) -> Result<&str, fmt::Error> {
    match str::from_utf8(value.as_bytes()) {
        Ok(value) => Ok(value),
        Err(_invalid) => Err(fmt::Error),
    }
}

fn fmt_ascii_value(value: &FieldValue, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    ascii_value(value).and_then(|value| f.write_str(value))
}

pub(super) fn fmt_ascii_values<'a>(values: impl IntoIterator<Item = FieldValueRef<'a>>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut values = values.into_iter();
    if let Some(value) = values.next() {
        f.write_str(str::from_utf8(value.as_bytes()).map_err(|_invalid| fmt::Error)?)?;
    }
    for value in values {
        f.write_str(", ")?;
        f.write_str(str::from_utf8(value.as_bytes()).map_err(|_invalid| fmt::Error)?)?;
    }
    Ok(())
}

impl_from_str!(
    #[cfg(any(test, feature = "headers-negotiation"))]
    (AcceptOwned, Accept),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (AcceptEncodingOwned, AcceptEncoding),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (AcceptLanguageOwned, AcceptLanguage),
    #[cfg(any(test, feature = "headers-range"))]
    (AcceptRangesOwned, AcceptRanges),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlAllowCredentialsOwned, AccessControlAllowCredentials),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlAllowHeadersOwned, AccessControlAllowHeaders),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlAllowMethodsOwned, AccessControlAllowMethods),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlAllowOriginOwned, AccessControlAllowOrigin),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlExposeHeadersOwned, AccessControlExposeHeaders),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlMaxAgeOwned, AccessControlMaxAge),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlRequestHeadersOwned, AccessControlRequestHeaders),
    #[cfg(any(test, feature = "headers-cors"))]
    (AccessControlRequestMethodOwned, AccessControlRequestMethod),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (AllowOwned, Allow),
    #[cfg(any(test, feature = "headers-cache-control"))]
    (CacheControlOwned, CacheControl),
    #[cfg(any(test, feature = "headers-range"))]
    (ContentRangeOwned, ContentRange),
    #[cfg(any(test, feature = "headers-security"))]
    (ContentSecurityPolicyOwned, ContentSecurityPolicy),
    #[cfg(any(test, feature = "headers-content-type"))]
    (ContentTypeOwned, ContentType),
    #[cfg(any(test, feature = "headers-etag"))]
    (ETagOwned, ETag),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (HostOwned, Host),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfMatchOwned, IfMatch),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfModifiedSinceOwned, IfModifiedSince),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfNoneMatchOwned, IfNoneMatch),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfRangeOwned, IfRange),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfUnmodifiedSinceOwned, IfUnmodifiedSince),
    #[cfg(any(test, feature = "headers-conditional"))]
    (LastModifiedOwned, LastModified),
    #[cfg(any(test, feature = "headers-location"))]
    (LocationOwned, Location),
    #[cfg(any(test, feature = "headers-range"))]
    (RangeOwned, Range),
    #[cfg(any(test, feature = "headers-security"))]
    (ReferrerPolicyOwned, ReferrerPolicy),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketAcceptOwned, SecWebSocketAccept),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketExtensionsOwned, SecWebSocketExtensions),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketKeyOwned, SecWebSocketKey),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketProtocolOwned, SecWebSocketProtocol),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketVersionOwned, SecWebSocketVersion),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (ServerOwned, Server),
    #[cfg(any(test, feature = "headers-security"))]
    (StrictTransportSecurityOwned, StrictTransportSecurity),
    #[cfg(any(test, feature = "headers-user-agent"))]
    (UserAgentOwned, UserAgent),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (VaryOwned, Vary),
    #[cfg(any(test, feature = "headers-security"))]
    (XContentTypeOptionsOwned, XContentTypeOptions),
);

impl_ascii_display!(
    #[cfg(any(test, feature = "headers-range"))]
    (ContentRangeOwned, ContentRange),
    #[cfg(any(test, feature = "headers-negotiation"))]
    (HostOwned, Host),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfModifiedSinceOwned, IfModifiedSince),
    #[cfg(any(test, feature = "headers-conditional"))]
    (IfUnmodifiedSinceOwned, IfUnmodifiedSince),
    #[cfg(any(test, feature = "headers-conditional"))]
    (LastModifiedOwned, LastModified),
    #[cfg(any(test, feature = "headers-range"))]
    (RangeOwned, Range),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketAcceptOwned, SecWebSocketAccept),
    #[cfg(any(test, feature = "headers-websocket"))]
    (SecWebSocketKeyOwned, SecWebSocketKey),
    #[cfg(any(test, feature = "headers-security"))]
    (XContentTypeOptionsOwned, XContentTypeOptions),
);

macro_rules! impl_ascii_values_display {
    ($($(#[$meta:meta])* $owned:ty),+ $(,)?) => {
        $(
            $(#[$meta])*
            impl std::fmt::Display for $owned {
                #[inline]
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    fmt_ascii_values(self.values(), f)
                }
            }
        )+
    };
}

impl_ascii_values_display!(
    #[cfg(any(test, feature = "headers-negotiation"))]
    AllowOwned,
    #[cfg(any(test, feature = "headers-negotiation"))]
    VaryOwned,
);

fn error(name: &'static FieldName, kind: DecodeErrorKind) -> DecodeError {
    DecodeError::new(name, kind)
}

pub(super) fn invalid_syntax(name: &'static FieldName) -> DecodeError {
    error(name, DecodeErrorKind::InvalidSyntax)
}

/// Trims leading and trailing optional whitespace (`' '` and `'\t'`).
pub(super) fn trim_ows(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|byte| !matches!(byte, b' ' | b'\t')).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[inline]
pub(super) fn has_non_ows(bytes: &[u8]) -> bool {
    match bytes.first() {
        Some(b' ' | b'\t') => bytes[1..].iter().any(|byte| !matches!(byte, b' ' | b'\t')),
        Some(_) => true,
        None => false,
    }
}

pub(super) fn value_from_bytes(name: &'static FieldName, bytes: Vec<u8>) -> Result<FieldValue, DecodeError> {
    match FieldValue::try_from(bytes) {
        Ok(value) => Ok(value),
        Err(_invalid) => Err(invalid_syntax(name)),
    }
}

// External monomorphizations are integration-tested; LLVM also emits an uncallable template.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn normalized_comma_value<'a>(
    name: &'static FieldName,
    mut items: impl Iterator<Item = &'a [u8]>,
) -> Result<Option<FieldValue>, DecodeError> {
    normalized_comma_value_impl(name, &mut items)
}

fn normalized_comma_value_impl(
    name: &'static FieldName,
    items: &mut dyn Iterator<Item = &[u8]>,
) -> Result<Option<FieldValue>, DecodeError> {
    let mut wire = Vec::new();
    for item in items {
        if item.is_empty() {
            continue;
        }
        if !wire.is_empty() {
            wire.extend_from_slice(b", ");
        }
        wire.extend_from_slice(item);
    }
    if wire.is_empty() {
        Ok(None)
    } else {
        value_from_bytes(name, wire).map(Some)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use std::fmt;

    use super::{FieldLinesIter, ascii_value, has_non_ows, normalized_comma_value, trim_ows, value_from_bytes};
    use crate::{FieldName, FieldValue, FieldValueRef};

    struct DisplayValues<'a>(&'a [FieldValueRef<'a>]);

    impl fmt::Display for DisplayValues<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            super::fmt_ascii_values(self.0.iter().copied(), f)
        }
    }

    #[test]
    fn field_lines_cover_every_storage_shape() {
        let mut lines = FieldLinesIter::empty();
        assert!(lines.is_empty());
        assert_eq!(lines.len(), 0);
        assert!(lines.iter().next().is_none());
        assert!(lines.iter_mut().next().is_none());

        lines.push(FieldValue::from_static("a"));
        assert_eq!(lines.len(), 1);
        lines.iter_mut().next().expect("one value").set_sensitive(true);
        assert!(lines.iter().next().expect("one value").is_sensitive());

        lines.push(FieldValue::from_static("b"));
        lines.push(FieldValue::from_static("c"));
        assert_eq!(
            lines.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(),
            [b"a".as_slice(), b"b".as_slice(), b"c".as_slice()]
        );
        assert_eq!(lines.clone().into_encoded().len(), 3);
        assert_eq!(lines.into_vec().len(), 3);

        assert_eq!(FieldLinesIter::one(FieldValue::from_static("x")).into_vec().len(), 1);
        assert!(FieldLinesIter::empty().into_vec().is_empty());
        assert_eq!(FieldLinesIter::one(FieldValue::from_static("x")).into_encoded().len(), 1);
        assert!(FieldLinesIter::empty().into_encoded().is_empty());
    }

    #[test]
    fn repeated_ascii_formatting_handles_empty_and_invalid_values() {
        assert_eq!(DisplayValues(&[]).to_string(), "");

        let invalid = [FieldValueRef::new(b"\xff")];
        let mut output = String::new();
        assert_eq!(
            fmt::write(&mut output, format_args!("{}", DisplayValues(&invalid))),
            Err(fmt::Error)
        );
    }

    #[test]
    fn whitespace_and_normalization_helpers_cover_edges() {
        assert_eq!(trim_ows(b"\t  value \t"), b"value");
        assert_eq!(trim_ows(b" \t"), b"");
        assert!(has_non_ows(b"value"));
        assert!(has_non_ows(b" \tvalue"));
        assert!(!has_non_ows(b" \t"));
        assert!(!has_non_ows(b""));

        let normalized = normalized_comma_value(&FieldName::CacheControl, [b"".as_slice(), b"one", b"", b"two"].into_iter())
            .expect("valid field bytes")
            .expect("nonempty normalized value");
        assert_eq!(normalized, "one, two");
        assert_eq!(
            normalized_comma_value(&FieldName::CacheControl, [b"".as_slice()].into_iter()),
            Ok(None)
        );
        assert!(value_from_bytes(&FieldName::CacheControl, vec![b'\n']).is_err());

        let content_range = "bytes 0-1/2".parse::<super::ContentRangeOwned>().expect("valid range");
        assert_eq!(content_range.to_string(), "bytes 0-1/2");
        assert!("\n".parse::<super::ContentRangeOwned>().is_err());

        let opaque = FieldValue::try_from(vec![0xff]).expect("obs-text is valid field data");
        assert!(ascii_value(&opaque).is_err());
    }
}
