// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded `application/x-www-form-urlencoded` request-body extraction.
//!
//! This module is available with the additive `form` feature. That feature
//! implies `route` and `query`; neither feature enables forms on its own.
//! Decoding reuses [`FromQuery`](crate::query::FromQuery), including its
//! percent/plus rules, schema behavior, structured errors, and resource
//! limits.
//!
//! The encoded bytes are buffered only for the duration of extraction.
//! Consequently, `T` must implement `FromQuery` for every possible input
//! lifetime. This higher-ranked bound permits owned schemas such as `String`,
//! numeric, optional, and repeated fields while preventing references into
//! the temporary body buffer from escaping.
//!
//! ```
//! use routerama::query::FromQuery;
//! use routerama::route::form::Form;
//!
//! #[derive(FromQuery)]
//! struct Registration {
//!     name: String,
//!     newsletter: Option<bool>,
//!     topic: Vec<String>,
//! }
//!
//! fn accepts_owned(_: Form<Registration, 1024>) {}
//! ```
//!
//! Borrowed query schemas intentionally do not satisfy the form extractor's
//! ownership contract:
//!
//! ```compile_fail
//! use bytes::Bytes;
//! use http_body_util::Empty;
//! use routerama::query::FromQuery;
//! use routerama::route::form::Form;
//! use routerama::route::FromRequestBody;
//!
//! #[derive(FromQuery)]
//! struct Borrowed<'form> {
//!     value: &'form str,
//! }
//!
//! fn require_extractor<T>()
//! where
//!     Form<T, 64>: FromRequestBody<(), Empty<Bytes>>,
//! {}
//!
//! require_extractor::<Borrowed<'static>>();
//! ```

use core::fmt;
use core::ops::Deref;

use bytes::Bytes;
use http::StatusCode;
use http::header::HeaderValue;
use http::request::Parts;
use http_body::Body as HttpBody;

use super::extract::{BodyStateWitnessBody, collect_body};
use super::predicate::{ContentTypeCardinality, parse_content_type, single_content_type};
use super::{BodyRejection, BodyStateWitness, FromRequestBody, InvalidUtf8Error};
use crate::response::{Body, IntoResponse, Response};

/// A decoded form request body with an explicit maximum encoded size.
///
/// `LIMIT` is measured in bytes before UTF-8 and form decoding. The request
/// must contain exactly one valid `application/x-www-form-urlencoded`
/// `Content-Type`; legal media-type parameters are accepted and ignored.
///
/// `T` must be owned with respect to the encoded form input. In particular, a
/// schema containing `&str` fields cannot satisfy the extractor's
/// `for<'form> FromQuery<'form>` bound. Use `String` or another owned field
/// type instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Form<T, const LIMIT: usize>(pub T);

impl<T, const LIMIT: usize> Form<T, LIMIT> {
    /// Consumes the wrapper and returns the decoded value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, const LIMIT: usize> Deref for Form<T, LIMIT> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B, T, const LIMIT: usize> FromRequestBody<S, B> for Form<T, LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = Bytes>,
    T: for<'form> crate::query::FromQuery<'form>,
{
    type Rejection = FormRejection<B::Error>;

    fn from_request_body(parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        let content_type = validate_content_type(parts);
        async move {
            content_type.map_err(FormRejection::UnsupportedMediaType)?;
            let bytes = collect_body::<B, LIMIT>(body).await.map_err(FormRejection::Body)?;
            let text = core::str::from_utf8(&bytes).map_err(|error| FormRejection::InvalidUtf8(InvalidUtf8Error::new(error)))?;
            T::from_query(text)
                .map(Self)
                .map_err(|error| FormRejection::Malformed(FormDecodeError::new(error)))
        }
    }
}

impl<S, T, E, const LIMIT: usize> BodyStateWitness<S, FormRejection<E>> for Form<T, LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<E>;
}

/// Why a request's `Content-Type` was not acceptable for a form body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormContentTypeError {
    /// The request did not include `Content-Type`.
    Missing,
    /// The request included more than one `Content-Type` value.
    Multiple {
        /// The number of header values supplied.
        count: usize,
    },
    /// The supplied header was not a valid media type.
    Malformed(HeaderValue),
    /// The supplied valid media type was not form-urlencoded.
    Unsupported(HeaderValue),
}

impl fmt::Display for FormContentTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("form request body requires a Content-Type header"),
            Self::Multiple { count } => write!(f, "form request body requires one Content-Type header, but received {count}"),
            Self::Malformed(value) => write!(f, "form request body received malformed Content-Type {value:?}"),
            Self::Unsupported(value) => write!(f, "form request body does not support Content-Type {value:?}"),
        }
    }
}

impl core::error::Error for FormContentTypeError {}

/// A failure reported by Routerama's query codec while decoding a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormDecodeError {
    error: crate::query::Error,
}

impl FormDecodeError {
    fn new(error: crate::query::Error) -> Self {
        Self { error }
    }

    /// Returns the detailed query-codec error.
    #[must_use]
    pub const fn error(&self) -> &crate::query::Error {
        &self.error
    }

    /// Consumes the wrapper and returns the detailed query-codec error.
    #[must_use]
    pub const fn into_inner(self) -> crate::query::Error {
        self.error
    }
}

impl fmt::Display for FormDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request body contains a malformed form: {}", self.error)
    }
}

impl core::error::Error for FormDecodeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A bounded form request-body rejection.
///
/// Missing, duplicate, malformed, and unsupported content types become `415
/// Unsupported Media Type`. An exceeded byte limit becomes `413 Payload Too
/// Large`. Transport, UTF-8, and query-codec failures become `400 Bad
/// Request`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormRejection<E> {
    /// The request lacks one valid form `Content-Type`.
    UnsupportedMediaType(FormContentTypeError),
    /// Buffering or body transport failed.
    Body(BodyRejection<E>),
    /// The bounded body was not valid UTF-8.
    InvalidUtf8(InvalidUtf8Error),
    /// The UTF-8 body could not be decoded through `FromQuery`.
    Malformed(FormDecodeError),
}

impl<E> fmt::Display for FormRejection<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMediaType(error) => error.fmt(f),
            Self::Body(error) => error.fmt(f),
            Self::InvalidUtf8(error) => error.fmt(f),
            Self::Malformed(error) => error.fmt(f),
        }
    }
}

impl<E> core::error::Error for FormRejection<E>
where
    E: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::UnsupportedMediaType(error) => Some(error),
            Self::Body(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
            Self::Malformed(error) => Some(error),
        }
    }
}

impl<E> IntoResponse for FormRejection<E> {
    type Body = Body;

    fn into_response(self) -> Response<Body> {
        match self {
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response(),
            Self::Body(error) => error.into_response(),
            Self::InvalidUtf8(_) | Self::Malformed(_) => StatusCode::BAD_REQUEST.into_response(),
        }
    }
}

fn validate_content_type(parts: &Parts) -> Result<(), FormContentTypeError> {
    let value = match single_content_type(&parts.headers) {
        Ok(value) => value,
        Err(ContentTypeCardinality::Missing) => return Err(FormContentTypeError::Missing),
        Err(ContentTypeCardinality::Multiple(count)) => return Err(FormContentTypeError::Multiple { count }),
    };
    let Some(media_type) = parse_content_type(value.as_bytes()) else {
        return Err(FormContentTypeError::Malformed(value.clone()));
    };
    if media_type.top_level.eq_ignore_ascii_case(b"application") && media_type.subtype.eq_ignore_ascii_case(b"x-www-form-urlencoded") {
        Ok(())
    } else {
        Err(FormContentTypeError::Unsupported(value.clone()))
    }
}
