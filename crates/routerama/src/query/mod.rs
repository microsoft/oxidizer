// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed, bounded query-string parsing and production.
//!
//! [`FromQuery`] and [`ToQuery`] follow
//! `application/x-www-form-urlencoded` percent and `+` rules. Derives support
//! scalar, optional, repeated, flattened, renamed, aliased, defaulted, and
//! skipped fields, plus compatible `serde` attributes.
//!
//! [`QueryLimits`] bounds parsing and production. The `form` feature reuses
//! [`FromQuery`] for bounded request bodies and requires owned output so
//! references cannot escape the temporary body buffer.

mod decode_fields;
mod decoded;
mod encode_fields;
mod encoder;
mod error;
mod error_kind;
mod from_query;
mod parser;
mod query_decoder;
mod query_limits;
mod scan;
mod to_query;

use decode_fields::DecodeFields;
use decoded::Decoded;
use encode_fields::EncodeFields;
use encoder::Encoder;
pub use error::Error;
pub use error_kind::ErrorKind;
pub use from_query::FromQuery;
use query_decoder::QueryDecoder;
pub use query_limits::QueryLimits;
/// Derives direct query-string decoding for a named-field struct.
///
/// Supports borrowed or owned scalar fields, optional fields, repeated
/// [`Vec`](alloc::vec::Vec) fields, flattening, renaming, aliases, defaults,
/// skipped fields, and unknown-field rejection.
pub use routerama_macros::FromQuery;
/// Derives direct query-string encoding for a named-field struct.
///
/// Fields are emitted in declaration order; optional values are omitted and
/// repeated values emit one parameter per element.
pub use routerama_macros::ToQuery;
pub use to_query::ToQuery;

/// Runtime support referenced by generated query codecs.
#[doc(hidden)]
pub mod __private {
    /// Repeated query-field storage used by generated decoders.
    pub type Repeated<T> = alloc::vec::Vec<T>;

    pub use super::decode_fields::DecodeFields;
    pub use super::decoded::{Decoded, parse_cow, parse_owned, parse_value};
    pub use super::encode_fields::{
        EncodeFields, EncodedLengthHint, encoded_bool_length, encoded_i8_length, encoded_i16_length, encoded_i32_length,
        encoded_i64_length, encoded_i128_length, encoded_isize_length, encoded_str_length, encoded_u8_length, encoded_u16_length,
        encoded_u32_length, encoded_u64_length, encoded_u128_length, encoded_usize_length,
    };
    pub use super::encoder::Encoder;
    pub use super::error::Error;
    pub use super::parser::{
        ParsedPrimitive, RawValue, decode_value, discard_value, parse_bool, parse_borrowed, parse_i8, parse_i16, parse_i32, parse_i64,
        parse_i128, parse_isize, parse_u8, parse_u16, parse_u32, parse_u64, parse_u128, parse_usize,
    };
    pub use super::query_decoder::QueryDecoder;
    pub use super::query_limits::QueryLimits;
}
