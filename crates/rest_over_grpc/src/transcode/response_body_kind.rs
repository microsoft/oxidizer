// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ResponseBodyKind`] response-body mapping mode.

/// How the response message maps onto the HTTP response body (mirrors the
/// `HttpRule.response_body` field at runtime).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::ResponseBodyKind;
///
/// let whole = ResponseBodyKind::Whole;
/// assert_eq!(whole, ResponseBodyKind::Whole);
///
/// let field = ResponseBodyKind::Field("name");
/// assert_eq!(field, ResponseBodyKind::Field("name"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponseBodyKind {
    /// The whole response message is the body.
    Whole,
    /// A single named top-level field of the response message is the body.
    Field(&'static str),
}
