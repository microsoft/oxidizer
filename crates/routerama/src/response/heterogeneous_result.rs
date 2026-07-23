// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::into_response::IntoResponse;
use super::{DataEitherBody, Response};

/// An explicit heterogeneous-data [`Result`] response.
///
/// Ordinary `Result<T, E>` keeps one shared frame-data type and remains the
/// zero-overhead default. This wrapper maps differing success and error data
/// into [`EitherData`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeterogeneousResult<T, E>(pub Result<T, E>);

impl<T, E> From<Result<T, E>> for HeterogeneousResult<T, E> {
    fn from(result: Result<T, E>) -> Self {
        Self(result)
    }
}

impl<T, E> IntoResponse for HeterogeneousResult<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
{
    type Body = DataEitherBody<T::Body, E::Body>;

    fn into_response(self) -> Response<Self::Body> {
        match self.0 {
            Ok(value) => value.into_response().map(|body| DataEitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| DataEitherBody::Right { body }),
        }
    }
}
