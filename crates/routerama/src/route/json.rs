// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded JSON request-body extraction.
//!
//! This module is available with the additive `json` feature. That feature
//! implies `route`; `route`, `query`, and `resolve` do not enable JSON support.

use core::fmt;
use core::ops::Deref;

use bytes::Bytes;
use http::StatusCode;
use http::header::HeaderValue;
use http::request::Parts;
use http_body::Body as HttpBody;
use serde::de::DeserializeOwned;

use super::extract::{BodyStateWitnessBody, collect_body};
use super::predicate::{ContentTypeCardinality, parse_content_type, single_content_type};
use super::{BodyRejection, BodyStateWitness, FromRequestBody};
use crate::response::{Body, IntoResponse, Response};

/// A decoded JSON request body with an explicit maximum encoded size.
///
/// `LIMIT` is measured in bytes before deserialization. Extraction accepts
/// `application/json` and `application/*+json` media types, including media
/// type parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Json<T, const LIMIT: usize>(pub T);

impl<T, const LIMIT: usize> Json<T, LIMIT> {
    /// Consumes the wrapper and returns the decoded value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, const LIMIT: usize> Deref for Json<T, LIMIT> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B, T, const LIMIT: usize> FromRequestBody<S, B> for Json<T, LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = Bytes>,
    T: DeserializeOwned,
{
    type Rejection = JsonRejection<B::Error>;

    fn from_request_body(parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        let content_type = validate_content_type(parts);
        async move {
            content_type.map_err(JsonRejection::UnsupportedMediaType)?;
            let bytes = collect_body::<B, LIMIT>(body).await.map_err(JsonRejection::Body)?;
            serde_json::from_slice(&bytes)
                .map(Self)
                .map_err(|error| JsonRejection::Malformed(JsonDecodeError::new(error)))
        }
    }
}

impl<S, T, E, const LIMIT: usize> BodyStateWitness<S, JsonRejection<E>> for Json<T, LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<E>;
}

/// Why a request's `Content-Type` was not an acceptable JSON media type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonContentTypeError {
    /// The request did not include `Content-Type`.
    Missing,
    /// The request included more than one `Content-Type` value.
    Multiple {
        /// The number of header values supplied.
        count: usize,
    },
    /// The supplied value was not a supported JSON media type.
    Unsupported(HeaderValue),
}

impl fmt::Display for JsonContentTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("JSON request body requires a Content-Type header"),
            Self::Multiple { count } => write!(f, "JSON request body requires one Content-Type header, but received {count}"),
            Self::Unsupported(value) => write!(f, "JSON request body does not support Content-Type {value:?}"),
        }
    }
}

impl core::error::Error for JsonContentTypeError {}

/// A JSON deserialization failure.
#[derive(Debug)]
pub struct JsonDecodeError {
    error: serde_json::Error,
}

impl JsonDecodeError {
    fn new(error: serde_json::Error) -> Self {
        Self { error }
    }

    /// Returns the concrete `serde_json` error.
    #[must_use]
    pub const fn error(&self) -> &serde_json::Error {
        &self.error
    }

    /// Consumes the wrapper and returns the concrete `serde_json` error.
    #[must_use]
    pub fn into_inner(self) -> serde_json::Error {
        self.error
    }
}

impl fmt::Display for JsonDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request body contains malformed JSON: {}", self.error)
    }
}

impl core::error::Error for JsonDecodeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A bounded JSON request-body rejection.
///
/// Unsupported media types become `415 Unsupported Media Type`; malformed
/// JSON and body transport failures become `400 Bad Request`; and an exceeded
/// byte limit becomes `413 Payload Too Large`.
#[derive(Debug)]
pub enum JsonRejection<E> {
    /// The request lacks one supported JSON `Content-Type`.
    UnsupportedMediaType(JsonContentTypeError),
    /// Buffering or body transport failed.
    Body(BodyRejection<E>),
    /// The bounded body could not be decoded as JSON.
    Malformed(JsonDecodeError),
}

impl<E> fmt::Display for JsonRejection<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMediaType(error) => error.fmt(f),
            Self::Body(error) => error.fmt(f),
            Self::Malformed(error) => error.fmt(f),
        }
    }
}

impl<E> core::error::Error for JsonRejection<E>
where
    E: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::UnsupportedMediaType(error) => Some(error),
            Self::Body(error) => Some(error),
            Self::Malformed(error) => Some(error),
        }
    }
}

impl<E> IntoResponse for JsonRejection<E> {
    type Body = Body;

    fn into_response(self) -> Response<Body> {
        match self {
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response(),
            Self::Body(error) => error.into_response(),
            Self::Malformed(_) => StatusCode::BAD_REQUEST.into_response(),
        }
    }
}

fn validate_content_type(parts: &Parts) -> Result<(), JsonContentTypeError> {
    let value = match single_content_type(&parts.headers) {
        Ok(value) => value,
        Err(ContentTypeCardinality::Missing) => return Err(JsonContentTypeError::Missing),
        Err(ContentTypeCardinality::Multiple(count)) => return Err(JsonContentTypeError::Multiple { count }),
    };
    if is_json_content_type(value) {
        Ok(())
    } else {
        Err(JsonContentTypeError::Unsupported(value.clone()))
    }
}

fn is_json_content_type(value: &HeaderValue) -> bool {
    let Some(media_type) = parse_content_type(value.as_bytes()) else {
        return false;
    };
    if !media_type.top_level.eq_ignore_ascii_case(b"application") {
        return false;
    }
    if media_type.subtype.eq_ignore_ascii_case(b"json") {
        return true;
    }
    let subtype = media_type.subtype;
    subtype.len() > b"+json".len() && subtype[subtype.len() - b"+json".len()..].eq_ignore_ascii_case(b"+json")
}
