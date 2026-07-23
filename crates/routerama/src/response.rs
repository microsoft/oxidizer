// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP body types and typed response composition.
//!
//! [`IntoResponse`] retains a concrete [`http_body::Body`] and its native
//! frame-data type.
//! [`Body`] provides a zero-or-one-frame body, [`EitherBody`] combines finite
//! alternatives without allocation, and [`BoxBody`] or [`SendBoxBody`] provide
//! explicit erased boundaries.
//!
//! [`IntoResponseParts`] applies fallible metadata to a body-free
//! [`ResponseParts`]. Tuple metadata is applied right to left, so the leftmost
//! status or duplicate header wins. A failure short-circuits and returns its
//! own response; concrete success and rejection bodies remain unboxed.
//!
//! [`json_body_template!`] and [`html_body_template!`] render fixed
//! boilerplate plus typed dynamic slots into one exactly sized allocation.

#[doc(hidden)]
#[path = "response/template.rs"]
pub mod __template;

mod body;
#[cfg(feature = "bytesbuf")]
pub mod bytesbuf;
mod heterogeneous_result;
mod into_response;
mod into_response_parts;
mod response_parts;
mod static_bytes;
mod static_text;

pub use body::{
    Body, BoxBody, BoxBodyError, DataEitherBody, EitherBody, EitherBodyError, EitherData, NeverBody, SendBoxBody, SendBoxBodyError,
};
pub use heterogeneous_result::HeterogeneousResult;
pub use into_response::IntoResponse;
pub use into_response_parts::IntoResponseParts;
pub use response_parts::ResponseParts;
pub use static_bytes::StaticBytes;
pub use static_text::StaticText;

/// Renders an HTML body template with escaped text-content slots.
///
/// Static fragments must be string literals. `text` slots escape `&`, `<`,
/// `>`, `"`, and `'`, so dynamic text cannot introduce markup. Slot
/// expressions are evaluated exactly once. A dynamic template allocates one
/// exactly sized contiguous buffer; a fully static template allocates nothing.
///
/// ```
/// use routerama::response::html_body_template;
///
/// let heading = "<Routerama>";
/// let body = html_body_template!(
///     heading = text(heading);
///     "<h1>", heading, "</h1>"
/// );
///
/// assert_eq!(body.as_bytes(), b"<h1>&lt;Routerama&gt;</h1>");
/// ```
pub use crate::__routerama_html_body_template as html_body_template;
/// Renders a JSON body template with integer and escaped-string slots.
///
/// Static fragments must be string literals. Every declared slot is one
/// complete JSON value: `number` accepts integer primitives and `string`
/// quotes and escapes a string-like value. Slot expressions are evaluated
/// exactly once. A dynamic template allocates one exactly sized contiguous
/// buffer; a fully static template allocates nothing.
///
/// ```
/// use routerama::response::json_body_template;
///
/// let message = "quote: \"routerama\"\nline";
/// let body = json_body_template!(
///     id = number(42_u64),
///     message = string(message);
///     r#"{"id":"#, id, r#","message":"#, message, "}"
/// );
///
/// assert_eq!(
///     body.as_bytes(),
///     br#"{"id":42,"message":"quote: \"routerama\"\nline"}"#
/// );
/// ```
///
/// Dynamic fragments without a slot type are rejected:
///
/// ```compile_fail
/// use routerama::response::json_body_template;
///
/// let raw = r#""injected":true"#;
/// let _body = json_body_template!(
///     value = raw(raw);
///     "{", value, "}"
/// );
/// ```
pub use crate::__routerama_json_body_template as json_body_template;

/// An HTTP response, using Routerama's fixed [`Body`] by default.
pub type Response<B = Body> = http::Response<B>;
