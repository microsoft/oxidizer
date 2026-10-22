// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded JSON request-body extraction and JSON response bodies.
//!
//! [`Json`] decodes a request body up to an explicit limit. [`JsonBody`]
//! serializes a value for a response.
//!
//! This module is available with the additive `json` feature. That feature
//! implies `route`; `route`, `query`, and `resolve` do not enable JSON support.

use core::fmt;
use core::ops::Deref;

use bytes::Bytes;
use http::StatusCode;
use http::header::{CONTENT_TYPE, HeaderValue};
use http::request::Parts;
use http_body::Body as HttpBody;
use serde::Serialize;
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
    pub(super) fn new(error: serde_json::Error) -> Self {
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

/// A serialized JSON response body.
///
/// Serialization happens in [`JsonBody::new`], so converting the value into a
/// response cannot fail and no encoding policy is baked into this type. A
/// handler that returns `Result<JsonBody, JsonEncodeError>` composes through
/// the [`IntoResponse`] implementation for `Result`, which turns an encoding
/// failure into `500 Internal Server Error`. A handler that wants a different
/// outcome maps the error to its own type instead.
///
/// # Examples
///
/// ```
/// use routerama::response::IntoResponse;
/// use routerama::route::json::JsonBody;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Reply {
///     id: u32,
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let body = JsonBody::new(&Reply { id: 7 })?;
/// assert_eq!(body.as_bytes(), &b"{\"id\":7}"[..]);
///
/// let response = body.into_response();
/// assert_eq!(response.headers()["content-type"], "application/json");
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonBody {
    bytes: Bytes,
}

impl JsonBody {
    /// Serializes `value` as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when `value`'s [`Serialize`] implementation fails.
    /// `serde_json` reports non-string map keys and non-finite floating-point
    /// values this way, as does any user implementation that returns an error.
    pub fn new<T>(value: &T) -> Result<Self, JsonEncodeError>
    where
        T: Serialize + ?Sized,
    {
        match serde_json::to_vec(value) {
            Ok(bytes) => Ok(Self { bytes: Bytes::from(bytes) }),
            Err(error) => Err(JsonEncodeError { error }),
        }
    }

    /// Returns the serialized JSON.
    #[must_use]
    pub const fn as_bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// Consumes the wrapper and returns the serialized JSON.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }
}

impl IntoResponse for JsonBody {
    type Body = Body;

    fn into_response(self) -> Response<Body> {
        let mut response = Response::new(Body::from_bytes(self.bytes));
        _ = response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    }
}

/// A JSON serialization failure.
///
/// Becomes `500 Internal Server Error` with an empty body.
#[derive(Debug)]
pub struct JsonEncodeError {
    error: serde_json::Error,
}

impl JsonEncodeError {
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

impl fmt::Display for JsonEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "response value could not be serialized as JSON: {}", self.error)
    }
}

impl core::error::Error for JsonEncodeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl IntoResponse for JsonEncodeError {
    type Body = Body;

    fn into_response(self) -> Response<Body> {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

pub(super) fn validate_content_type(parts: &Parts) -> Result<(), JsonContentTypeError> {
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
