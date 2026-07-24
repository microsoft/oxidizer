// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`RequestBodyKind`] request-body mapping mode.

/// How the HTTP request body maps onto the request message (mirrors the
/// `HttpRule.body` field at runtime).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::RequestBodyKind;
///
/// assert_eq!(RequestBodyKind::None, RequestBodyKind::None);
/// assert_eq!(RequestBodyKind::Whole, RequestBodyKind::Whole);
/// assert_eq!(
///     RequestBodyKind::Field("book"),
///     RequestBodyKind::Field("book")
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestBodyKind {
    /// No request body is consumed.
    None,
    /// The entire body is the request message (`body: "*"`).
    Whole,
    /// The body maps onto a single named top-level field of the request message.
    Field(&'static str),
}
