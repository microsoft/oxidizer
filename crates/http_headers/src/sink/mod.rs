// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Definitions involved in writing fields to field containers.
//!
//! A container implements [`FieldSink`] to accept field lines, and
//! `FieldSinkExt` provides the feature-gated fluent setters most callers use. A
//! field reaches a sink through [`FieldEncoder`], which emits one value per
//! field line into a [`FieldEncodeOutput`]; [`EncodedValues`] is the owned
//! form those values collect into.
//!
//! The `http` cargo feature implements [`FieldSink`] for `http::HeaderMap`.
//! Implement it yourself only when integrating a different container.
//!
//! [`crate::FieldSensitivity`] is a crate-root value type,
//! rather than a second export from this module.
//!
//! ```compile_fail
//! use http_headers::sink::FieldSensitivity;
//! ```

mod encoded_values;
pub(crate) mod field_encoder;
mod field_sink;
#[cfg(any(
    test,
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
mod field_sink_ext;
mod insert_error;

#[doc(inline)]
pub use encoded_values::{EncodedValues, EncodedValuesIntoIter, EncodedValuesIter, EncodedValuesIterMut};
#[doc(inline)]
pub(crate) use field_encoder::FieldSensitivity;
#[doc(inline)]
pub use field_encoder::{FieldEncodeOutput, FieldEncoder, FieldValueWriter, U64Encoder, ValueRefsEncoder};
#[doc(inline)]
pub use field_sink::FieldSink;
#[cfg(any(
    test,
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
#[doc(inline)]
pub use field_sink_ext::FieldSinkExt;
#[doc(inline)]
pub use insert_error::{InsertError, InsertErrorKind};
