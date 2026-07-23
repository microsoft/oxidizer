// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::String;
use alloc::vec::Vec;
use core::convert::Infallible;

use bytes::Bytes;
use http::StatusCode;
use http::header::{CONTENT_TYPE, HeaderValue};
use http_body::Body as HttpBody;

use super::into_response_parts::IntoResponseParts;
use super::response_parts::ResponseParts;
use super::{Body, BoxBody, EitherBody, NeverBody, Response};

/// Converts a handler result or rejection into an HTTP response.
pub trait IntoResponse {
    /// The concrete body retained by this response conversion.
    ///
    /// Generated routers combine the finite set of these body types into one
    /// service-specific sum without normalizing their frame data.
    type Body: HttpBody;

    /// Converts this value into a response.
    #[must_use]
    fn into_response(self) -> Response<Self::Body>;
}

impl<B> IntoResponse for http::Response<B>
where
    B: HttpBody,
{
    type Body = B;

    fn into_response(self) -> Response<Self::Body> {
        self
    }
}

impl IntoResponse for Body {
    type Body = Self;

    fn into_response(self) -> Response<Self::Body> {
        Response::new(self)
    }
}

impl<D> IntoResponse for BoxBody<D>
where
    D: bytes::Buf,
{
    type Body = Self;

    fn into_response(self) -> Response<Self::Body> {
        Response::new(self)
    }
}

impl IntoResponse for Bytes {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::from(self).into_response()
    }
}

impl IntoResponse for Vec<u8> {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::from(self).into_response()
    }
}

impl IntoResponse for String {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        text_response(Body::from(self))
    }
}

impl IntoResponse for &str {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        text_response(Body::from(self))
    }
}

impl IntoResponse for () {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::empty().into_response()
    }
}

impl IntoResponse for StatusCode {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Body::empty().into_response();
        *response.status_mut() = self;
        response
    }
}

impl IntoResponse for Infallible {
    type Body = NeverBody;

    fn into_response(self) -> Response<Self::Body> {
        match self {}
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
    E::Body: HttpBody<Data = <T::Body as HttpBody>::Data>,
{
    type Body = EitherBody<T::Body, E::Body>;

    fn into_response(self) -> Response<Self::Body> {
        match self {
            Ok(value) => value.into_response().map(|body| EitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| EitherBody::Right { body }),
        }
    }
}

impl<P, R> IntoResponse for (P, R)
where
    P: IntoResponseParts,
    R: IntoResponse,
    <P::Error as IntoResponse>::Body: HttpBody<Data = <R::Body as HttpBody>::Data>,
{
    type Body = EitherBody<R::Body, <P::Error as IntoResponse>::Body>;

    fn into_response(self) -> Response<Self::Body> {
        let (parts, value) = self;
        let (response_parts, body) = value.into_response().into_parts();
        match parts.into_response_parts(ResponseParts::new(response_parts)) {
            Ok(response_parts) => Response::from_parts(response_parts.into_inner(), EitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| EitherBody::Right { body }),
        }
    }
}

impl<P1, P2, R> IntoResponse for (P1, P2, R)
where
    P1: IntoResponseParts,
    P2: IntoResponseParts,
    R: IntoResponse,
    <P2::Error as IntoResponse>::Body: HttpBody<Data = <R::Body as HttpBody>::Data>,
    <P1::Error as IntoResponse>::Body: HttpBody<Data = <R::Body as HttpBody>::Data>,
{
    type Body = EitherBody<R::Body, EitherBody<<P2::Error as IntoResponse>::Body, <P1::Error as IntoResponse>::Body>>;

    fn into_response(self) -> Response<Self::Body> {
        let (first, second, value) = self;
        let (response_parts, body) = value.into_response().into_parts();
        let response_parts = match second.into_response_parts(ResponseParts::new(response_parts)) {
            Ok(response_parts) => response_parts,
            Err(error) => {
                return error.into_response().map(|body| EitherBody::Right {
                    body: EitherBody::Left { body },
                });
            }
        };
        match first.into_response_parts(response_parts) {
            Ok(response_parts) => Response::from_parts(response_parts.into_inner(), EitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| EitherBody::Right {
                body: EitherBody::Right { body },
            }),
        }
    }
}

pub(super) fn text_response(body: Body) -> Response {
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}
