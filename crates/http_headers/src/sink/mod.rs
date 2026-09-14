// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Definitions involved in writing fields to field containers.
//!
//! A container implements [`FieldSink`] to accept field lines, and
//! [`FieldSinkExt`] provides the fluent setters most callers use. A
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
#[doc(inline)]
pub use field_sink_ext::FieldSinkExt;
#[doc(inline)]
pub use insert_error::InsertError;
