// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`BodyKind`] request-body mapping mode.

/// How the HTTP request body maps onto the request message (mirrors the
/// `HttpRule.body` field at runtime).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::transcode::BodyKind;
///
/// assert_eq!(BodyKind::None, BodyKind::None);
/// assert_eq!(BodyKind::Whole, BodyKind::Whole);
/// assert_eq!(BodyKind::Field("book"), BodyKind::Field("book"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// No request body is consumed.
    None,
    /// The entire body is the request message (`body: "*"`).
    Whole,
    /// The body maps onto a single named top-level field of the request message.
    Field(&'static str),
}
