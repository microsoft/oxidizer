// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use std::iter;

use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use crate::DecodeMode;
#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use crate::Field;
#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-conditional",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use crate::SingleValueField;
#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use crate::headers::*;
use crate::sink::{EncodedValues, FieldSensitivity};
#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use crate::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES};
use crate::{DecodeErrorKind, FieldName, FieldValue, FieldValueRef};

const FIELD_VALUE_FIELDS: &[&str] = &["bytes", "sensitivity"];
// Bound speculative allocation from untrusted Serde size hints.
const SIZE_HINT_RESERVE_LIMIT: usize = 1_024;

#[derive(Serialize)]
struct FieldValueRepr<'a> {
    bytes: &'a [u8],
    sensitivity: FieldSensitivity,
}

#[derive(Clone, Copy)]
enum FieldValueField {
    Bytes,
    Sensitivity,
    Other,
}

impl<'de> Deserialize<'de> for FieldValueField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;

        impl Visitor<'_> for FieldVisitor {
            type Value = FieldValueField;

            #[cfg_attr(coverage_nightly, coverage(off))]
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("`bytes` or `sensitivity`")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(match v {
                    "bytes" => FieldValueField::Bytes,
                    "sensitivity" => FieldValueField::Sensitivity,
                    _ => FieldValueField::Other,
                })
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(match v {
                    b"bytes" => FieldValueField::Bytes,
                    b"sensitivity" => FieldValueField::Sensitivity,
                    _ => FieldValueField::Other,
                })
            }
        }

        deserializer.deserialize_identifier(FieldVisitor)
    }
}

struct FieldBytesSeed {
    limit: Option<usize>,
}

impl<'de> DeserializeSeed<'de> for FieldBytesSeed {
    type Value = Vec<u8>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        struct BytesVisitor {
            limit: Option<usize>,
        }

        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            #[cfg_attr(coverage_nightly, coverage(off))]
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.limit {
                    Some(limit) => write!(formatter, "at most {limit} field-value bytes"),
                    None => formatter.write_str("field-value bytes"),
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let capacity = seq
                    .size_hint()
                    .unwrap_or(0)
                    .min(self.limit.unwrap_or(SIZE_HINT_RESERVE_LIMIT))
                    .min(SIZE_HINT_RESERVE_LIMIT);
                let mut bytes = Vec::with_capacity(capacity);
                while self.limit.is_none_or(|limit| bytes.len() < limit) {
                    match seq.next_element()? {
                        Some(byte) => bytes.push(byte),
                        None => return Ok(bytes),
                    }
                }
                if seq.next_element::<IgnoredAny>()?.is_some() {
                    return Err(A::Error::custom(format_args!(
                        "{}: field-value byte budget exceeded",
                        DecodeErrorKind::SourceLimitExceeded
                    )));
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_seq(BytesVisitor { limit: self.limit })
    }
}

struct FieldValueSeed {
    byte_limit: Option<usize>,
}

impl<'de> DeserializeSeed<'de> for FieldValueSeed {
    type Value = FieldValue;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        struct FieldValueVisitor {
            byte_limit: Option<usize>,
        }

        impl FieldValueVisitor {
            fn finish<E: serde::de::Error>(bytes: Option<Vec<u8>>, sensitivity: Option<FieldSensitivity>) -> Result<FieldValue, E> {
                let bytes = bytes.ok_or_else(|| E::missing_field("bytes"))?;
                let sensitivity = sensitivity.ok_or_else(|| E::missing_field("sensitivity"))?;
                FieldValue::try_from(bytes)
                    .map(|value| value.with_sensitivity(sensitivity))
                    .map_err(E::custom)
            }
        }

        impl<'de> Visitor<'de> for FieldValueVisitor {
            type Value = FieldValue;

            #[cfg_attr(coverage_nightly, coverage(off))]
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a field value with bytes and sensitivity")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let bytes = seq
                    .next_element_seed(FieldBytesSeed { limit: self.byte_limit })?
                    .ok_or_else(|| A::Error::invalid_length(0, &self))?;
                let sensitivity = seq.next_element()?.ok_or_else(|| A::Error::invalid_length(1, &self))?;
                Self::finish(Some(bytes), Some(sensitivity))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut bytes = None;
                let mut sensitivity = None;
                while let Some(field) = map.next_key()? {
                    match field {
                        FieldValueField::Bytes => {
                            if bytes.is_some() {
                                return Err(A::Error::duplicate_field("bytes"));
                            }
                            bytes = Some(map.next_value_seed(FieldBytesSeed { limit: self.byte_limit })?);
                        }
                        FieldValueField::Sensitivity => {
                            if sensitivity.is_some() {
                                return Err(A::Error::duplicate_field("sensitivity"));
                            }
                            sensitivity = Some(map.next_value()?);
                        }
                        FieldValueField::Other => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Self::finish(bytes, sensitivity)
            }
        }

        deserializer.deserialize_struct(
            "FieldValueRepr",
            FIELD_VALUE_FIELDS,
            FieldValueVisitor {
                byte_limit: self.byte_limit,
            },
        )
    }
}

#[derive(Clone, Copy)]
struct ListCounter {
    delimiter: u8,
    skip_empty: bool,
    backslash_escapes: bool,
    count: usize,
}

impl ListCounter {
    const fn new(delimiter: u8, skip_empty: bool, backslash_escapes: bool) -> Self {
        Self {
            delimiter,
            skip_empty,
            backslash_escapes,
            count: 0,
        }
    }

    fn observe(&mut self, name: &'static FieldName, bytes: &[u8]) -> Result<(), crate::DecodeError> {
        crate::source::update_list_item_count(
            name,
            bytes,
            self.delimiter,
            self.skip_empty,
            self.backslash_escapes,
            &mut self.count,
        )
    }
}

enum ListBudgets {
    None,
    One(ListCounter),
    Two(ListCounter, ListCounter),
}

impl ListBudgets {
    fn for_name(name: Option<&'static FieldName>) -> Self {
        let comma = || ListCounter::new(b',', true, true);
        let semicolon = || ListCounter::new(b';', false, true);
        match name {
            Some(
                &FieldName::CacheControl
                | &FieldName::Accept
                | &FieldName::AcceptEncoding
                | &FieldName::AcceptLanguage
                | &FieldName::Allow
                | &FieldName::Vary
                | &FieldName::AcceptRanges
                | &FieldName::Range
                | &FieldName::AccessControlAllowHeaders
                | &FieldName::AccessControlAllowMethods
                | &FieldName::AccessControlExposeHeaders
                | &FieldName::AccessControlRequestHeaders
                | &FieldName::ReferrerPolicy
                | &FieldName::SecWebSocketProtocol
                | &FieldName::SecWebSocketVersion,
            ) => Self::One(comma()),
            Some(&FieldName::ContentLength) => Self::One(ListCounter::new(b',', false, true)),
            Some(&FieldName::ContentType | &FieldName::StrictTransportSecurity) => Self::One(semicolon()),
            Some(&FieldName::IfMatch | &FieldName::IfNoneMatch) => Self::One(ListCounter::new(b',', true, false)),
            Some(&FieldName::SecWebSocketExtensions) => Self::Two(comma(), semicolon()),
            _ => Self::None,
        }
    }

    fn observe(&mut self, name: Option<&'static FieldName>, bytes: &[u8]) -> Result<(), crate::DecodeError> {
        let Some(name) = name else {
            return Ok(());
        };
        match self {
            Self::None => Ok(()),
            Self::One(counter) => counter.observe(name, bytes),
            Self::Two(first, second) => {
                first.observe(name, bytes)?;
                second.observe(name, bytes)
            }
        }
    }
}

struct FieldValuesVisitor {
    name: Option<&'static FieldName>,
    byte_limit: Option<usize>,
    line_limit: Option<usize>,
}

impl<'de> Visitor<'de> for FieldValuesVisitor {
    type Value = Vec<FieldValue>;

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line_limit {
            Some(limit) => write!(formatter, "at most {limit} field values"),
            None => formatter.write_str("a sequence of field values"),
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let capacity = seq
            .size_hint()
            .unwrap_or(0)
            .min(self.line_limit.unwrap_or(SIZE_HINT_RESERVE_LIMIT))
            .min(SIZE_HINT_RESERVE_LIMIT);
        let mut values = Vec::with_capacity(capacity);
        let mut total_bytes = 0_usize;
        let mut list_budgets = ListBudgets::for_name(self.name);
        while self.line_limit.is_none_or(|limit| values.len() < limit) {
            let remaining_bytes = self.byte_limit.map(|limit| limit - total_bytes);
            let Some(value) = seq.next_element_seed(FieldValueSeed {
                byte_limit: remaining_bytes,
            })?
            else {
                return Ok(values);
            };
            list_budgets.observe(self.name, value.as_bytes()).map_err(A::Error::custom)?;
            total_bytes = checked_total_bytes::<A::Error>(total_bytes, value.as_bytes().len())?;
            values.push(value);
        }
        if seq.next_element::<IgnoredAny>()?.is_some() {
            return Err(A::Error::custom(format_args!(
                "{}: field-value line budget exceeded",
                DecodeErrorKind::SourceLimitExceeded
            )));
        }
        Ok(values)
    }
}

fn checked_total_bytes<E: serde::de::Error>(total: usize, additional: usize) -> Result<usize, E> {
    total.checked_add(additional).ok_or_else(|| {
        E::custom(format_args!(
            "{}: aggregate field-value size overflow",
            DecodeErrorKind::SourceLimitExceeded
        ))
    })
}

fn deserialize_field_values<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<FieldValue>, D::Error> {
    deserializer.deserialize_seq(FieldValuesVisitor {
        name: None,
        byte_limit: None,
        line_limit: None,
    })
}

#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
fn deserialize_field_values_for<'de, D: Deserializer<'de>>(
    name: Option<&'static FieldName>,
    deserializer: D,
) -> Result<Vec<FieldValue>, D::Error> {
    deserializer.deserialize_seq(FieldValuesVisitor {
        name,
        byte_limit: Some(MAX_CUSTOM_FIELD_BYTES),
        line_limit: Some(MAX_CUSTOM_FIELD_LINES),
    })
}

#[derive(Clone, Copy)]
struct SerializedFieldValue<'a> {
    value: FieldValueRef<'a>,
    sensitivity: FieldSensitivity,
}

impl SerializedFieldValue<'_> {
    fn preserving(value: FieldValueRef<'_>) -> SerializedFieldValue<'_> {
        let sensitivity = if value.is_sensitive() {
            FieldSensitivity::Sensitive
        } else {
            FieldSensitivity::NonSensitive
        };
        SerializedFieldValue { value, sensitivity }
    }
}

impl Serialize for SerializedFieldValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        FieldValueRepr {
            bytes: self.value.as_bytes(),
            sensitivity: self.sensitivity,
        }
        .serialize(serializer)
    }
}

fn serialize_field_values<'a, S, I>(values: I, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    I: IntoIterator<Item = FieldValueRef<'a>>,
{
    serializer.collect_seq(values.into_iter().map(SerializedFieldValue::preserving))
}

impl Serialize for FieldName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FieldName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if let Ok(name) = Self::try_from_bytes(&value) {
            return Ok(name);
        }
        #[cfg(feature = "http")]
        if let Ok(name) = http::HeaderName::from_lowercase(value.as_bytes()) {
            return Ok(Self::from(name));
        }
        Err(D::Error::custom(crate::InvalidFieldName))
    }
}

impl Serialize for FieldValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SerializedFieldValue::preserving(self.as_field_value_ref()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FieldValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        FieldValueSeed { byte_limit: None }.deserialize(deserializer)
    }
}

impl Serialize for EncodedValues {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.iter().map(FieldValue::as_field_value_ref), serializer)
    }
}

impl<'de> Deserialize<'de> for EncodedValues {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_field_values(deserializer).map(Self::from_vec)
    }
}

#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
struct SerdeSource {
    values: Vec<FieldValue>,
}

#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
impl FieldSource for SerdeSource {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_slice(name, &self.values)
    }
}

#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-conditional",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
fn serialize_single_owned<H, S>(value: &H::Owned, serializer: S) -> Result<S::Ok, S::Error>
where
    H: SingleValueField,
    S: Serializer,
{
    serialize_field_values(iter::once(H::as_field_value(value).as_field_value_ref()), serializer)
}

#[cfg(any(
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-length",
    feature = "headers-content-type",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
fn deserialize_owned<'de, H, D>(deserializer: D) -> Result<H::Owned, D::Error>
where
    H: Field,
    D: Deserializer<'de>,
{
    let source = SerdeSource {
        values: deserialize_field_values_for(Some(H::name()), deserializer)?,
    };
    H::owned_with(&source, DecodeMode::Relaxed)
        .map_err(D::Error::custom)?
        .ok_or_else(|| D::Error::custom("an owned header must contain at least one field value"))
}

macro_rules! serde_deserialize_owned {
    ($($(#[$meta:meta])* ($header:ty, $owned:ty)),+ $(,)?) => {
        $(
            $(#[$meta])*
            impl<'de> Deserialize<'de> for $owned {
                fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    deserialize_owned::<$header, D>(deserializer)
                }
            }
        )+
    };
}

macro_rules! serde_single_owned {
    ($($(#[$meta:meta])* ($header:ty, $owned:ty)),+ $(,)?) => {
        $(
            $(#[$meta])*
            impl Serialize for $owned {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serialize_single_owned::<$header, S>(self, serializer)
                }
            }
        )+
    };
}

macro_rules! serde_values_owned {
    ($($(#[$meta:meta])* ($owned:ty, $method:ident)),+ $(,)?) => {
        $(
            $(#[$meta])*
            impl Serialize for $owned {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serialize_field_values(self.$method(), serializer)
                }
            }
        )+
    };
}

serde_single_owned!(
    #[cfg(feature = "headers-negotiation")]
    (Host, HostOwned),
    #[cfg(feature = "headers-negotiation")]
    (Server, ServerOwned),
    #[cfg(feature = "headers-range")]
    (ContentRange, ContentRangeOwned),
    #[cfg(feature = "headers-range")]
    (Range, RangeOwned),
    #[cfg(feature = "headers-etag")]
    (ETag, ETagOwned),
    #[cfg(feature = "headers-location")]
    (Location, LocationOwned),
    #[cfg(feature = "headers-user-agent")]
    (UserAgent, UserAgentOwned),
    #[cfg(feature = "headers-conditional")]
    (IfModifiedSince, IfModifiedSinceOwned),
    #[cfg(feature = "headers-conditional")]
    (IfUnmodifiedSince, IfUnmodifiedSinceOwned),
    #[cfg(feature = "headers-conditional")]
    (IfRange, IfRangeOwned),
    #[cfg(feature = "headers-conditional")]
    (LastModified, LastModifiedOwned),
    #[cfg(feature = "headers-security")]
    (StrictTransportSecurity, StrictTransportSecurityOwned),
    #[cfg(feature = "headers-security")]
    (XContentTypeOptions, XContentTypeOptionsOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketAccept, SecWebSocketAcceptOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketKey, SecWebSocketKeyOwned),
    #[cfg(feature = "headers-authorization")]
    (Authorization<Basic>, AuthorizationOwned<Basic>),
    #[cfg(feature = "headers-authorization")]
    (Authorization<Bearer>, AuthorizationOwned<Bearer>),
);

serde_values_owned!(
    #[cfg(feature = "headers-negotiation")]
    (AcceptOwned, values),
    #[cfg(feature = "headers-negotiation")]
    (AcceptEncodingOwned, values),
    #[cfg(feature = "headers-negotiation")]
    (AcceptLanguageOwned, values),
    #[cfg(feature = "headers-negotiation")]
    (AllowOwned, values),
    #[cfg(feature = "headers-negotiation")]
    (VaryOwned, values),
    #[cfg(feature = "headers-cors")]
    (AccessControlAllowHeadersOwned, field_values),
    #[cfg(feature = "headers-cors")]
    (AccessControlAllowMethodsOwned, field_values),
    #[cfg(feature = "headers-cors")]
    (AccessControlExposeHeadersOwned, field_values),
    #[cfg(feature = "headers-cors")]
    (AccessControlRequestHeadersOwned, field_values),
    #[cfg(feature = "headers-security")]
    (ContentSecurityPolicyOwned, field_values),
    #[cfg(feature = "headers-security")]
    (ReferrerPolicyOwned, field_values),
);

#[cfg(feature = "headers-cache-control")]
impl Serialize for CacheControlOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.field_values(), serializer)
    }
}

#[cfg(feature = "headers-range")]
impl Serialize for AcceptRangesOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.field_values(), serializer)
    }
}

#[cfg(feature = "headers-content-type")]
impl Serialize for ContentTypeOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(iter::once(self.field_value()), serializer)
    }
}

#[cfg(feature = "headers-conditional")]
impl Serialize for IfMatchOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.field_values(), serializer)
    }
}

#[cfg(feature = "headers-conditional")]
impl Serialize for IfNoneMatchOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.field_values(), serializer)
    }
}

#[cfg(feature = "headers-websocket")]
impl Serialize for SecWebSocketExtensionsOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.field_values(), serializer)
    }
}

#[cfg(feature = "headers-websocket")]
impl Serialize for SecWebSocketProtocolOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(self.field_values(), serializer)
    }
}

#[cfg(feature = "headers-websocket")]
impl Serialize for SecWebSocketVersionOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = self.field_value();
        serialize_field_values(iter::once(value.as_field_value_ref()), serializer)
    }
}

serde_deserialize_owned!(
    #[cfg(feature = "headers-cache-control")]
    (CacheControl, CacheControlOwned),
    #[cfg(feature = "headers-negotiation")]
    (Accept, AcceptOwned),
    #[cfg(feature = "headers-negotiation")]
    (AcceptEncoding, AcceptEncodingOwned),
    #[cfg(feature = "headers-negotiation")]
    (AcceptLanguage, AcceptLanguageOwned),
    #[cfg(feature = "headers-negotiation")]
    (Allow, AllowOwned),
    #[cfg(feature = "headers-negotiation")]
    (Host, HostOwned),
    #[cfg(feature = "headers-negotiation")]
    (Server, ServerOwned),
    #[cfg(feature = "headers-negotiation")]
    (Vary, VaryOwned),
    #[cfg(feature = "headers-range")]
    (AcceptRanges, AcceptRangesOwned),
    #[cfg(feature = "headers-range")]
    (ContentRange, ContentRangeOwned),
    #[cfg(feature = "headers-range")]
    (Range, RangeOwned),
    #[cfg(feature = "headers-etag")]
    (ETag, ETagOwned),
    #[cfg(feature = "headers-location")]
    (Location, LocationOwned),
    #[cfg(feature = "headers-user-agent")]
    (UserAgent, UserAgentOwned),
    #[cfg(feature = "headers-content-type")]
    (ContentType, ContentTypeOwned),
    #[cfg(feature = "headers-conditional")]
    (IfMatch, IfMatchOwned),
    #[cfg(feature = "headers-conditional")]
    (IfNoneMatch, IfNoneMatchOwned),
    #[cfg(feature = "headers-conditional")]
    (IfModifiedSince, IfModifiedSinceOwned),
    #[cfg(feature = "headers-conditional")]
    (IfUnmodifiedSince, IfUnmodifiedSinceOwned),
    #[cfg(feature = "headers-conditional")]
    (IfRange, IfRangeOwned),
    #[cfg(feature = "headers-conditional")]
    (LastModified, LastModifiedOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlAllowHeaders, AccessControlAllowHeadersOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlAllowMethods, AccessControlAllowMethodsOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlExposeHeaders, AccessControlExposeHeadersOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlRequestHeaders, AccessControlRequestHeadersOwned),
    #[cfg(feature = "headers-security")]
    (ContentSecurityPolicy, ContentSecurityPolicyOwned),
    #[cfg(feature = "headers-security")]
    (ReferrerPolicy, ReferrerPolicyOwned),
    #[cfg(feature = "headers-security")]
    (StrictTransportSecurity, StrictTransportSecurityOwned),
    #[cfg(feature = "headers-security")]
    (XContentTypeOptions, XContentTypeOptionsOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketAccept, SecWebSocketAcceptOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketExtensions, SecWebSocketExtensionsOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketKey, SecWebSocketKeyOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketProtocol, SecWebSocketProtocolOwned),
    #[cfg(feature = "headers-websocket")]
    (SecWebSocketVersion, SecWebSocketVersionOwned),
    #[cfg(feature = "headers-authorization")]
    (Authorization<Basic>, AuthorizationOwned<Basic>),
    #[cfg(feature = "headers-authorization")]
    (Authorization<Bearer>, AuthorizationOwned<Bearer>),
);

#[cfg(feature = "headers-cors")]
impl Serialize for AccessControlAllowCredentialsOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(iter::once(FieldValueRef::new(b"true")), serializer)
    }
}

#[cfg(feature = "headers-cors")]
impl Serialize for AccessControlMaxAgeOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = FieldValue::from(self.seconds());
        serialize_field_values(iter::once(value.as_field_value_ref()), serializer)
    }
}

#[cfg(feature = "headers-cors")]
impl Serialize for AccessControlAllowOriginOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(iter::once(self.field_value().as_field_value_ref()), serializer)
    }
}

#[cfg(feature = "headers-cors")]
impl Serialize for AccessControlRequestMethodOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_field_values(iter::once(self.field_value()), serializer)
    }
}

#[cfg(feature = "headers-content-length")]
impl Serialize for ContentLengthOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = FieldValue::from(self.get());
        serialize_field_values(iter::once(value.as_field_value_ref()), serializer)
    }
}

serde_deserialize_owned!(
    #[cfg(feature = "headers-cors")]
    (AccessControlAllowCredentials, AccessControlAllowCredentialsOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlMaxAge, AccessControlMaxAgeOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlAllowOrigin, AccessControlAllowOriginOwned),
    #[cfg(feature = "headers-cors")]
    (AccessControlRequestMethod, AccessControlRequestMethodOwned),
    #[cfg(feature = "headers-content-length")]
    (ContentLength, ContentLengthOwned),
);

#[cfg(feature = "headers-set-cookie")]
impl Serialize for SetCookieOwned {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(|value| SerializedFieldValue {
            value: value.as_field_value_ref(),
            sensitivity: FieldSensitivity::Sensitive,
        }))
    }
}

#[cfg(feature = "headers-set-cookie")]
impl<'de> Deserialize<'de> for SetCookieOwned {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = SerdeSource {
            values: deserialize_field_values_for(Some(SetCookie::name()), deserializer)?,
        };
        if source.values.is_empty() {
            return Ok(Self::new());
        }
        SetCookie::owned(&source)
            .map_err(D::Error::custom)?
            .ok_or_else(|| D::Error::custom("nonempty Set-Cookie values must decode as present"))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde::de::value::Error;

    use super::checked_total_bytes;

    #[test]
    fn aggregate_byte_count_preserves_representable_totals() {
        for (total, additional, expected) in [
            (0, 0, 0),
            (12, 34, 46),
            (0, usize::MAX, usize::MAX),
            (usize::MAX - 1, 1, usize::MAX),
        ] {
            assert_eq!(checked_total_bytes::<Error>(total, additional).unwrap(), expected);
        }
        assert_eq!(checked_total_bytes::<Error>(usize::MAX, 0).unwrap(), usize::MAX);
    }

    #[test]
    fn aggregate_byte_count_overflow_reports_source_limit_exceeded() {
        for (total, additional) in [(usize::MAX, 1), (1, usize::MAX), (usize::MAX, usize::MAX)] {
            assert_eq!(
                checked_total_bytes::<Error>(total, additional).unwrap_err().to_string(),
                "source limit exceeded: aggregate field-value size overflow"
            );
        }
    }
}
