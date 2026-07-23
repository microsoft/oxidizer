// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed parsing and production of URL query strings.
//!
//! Derive [`FromQuery`] for inbound query parameters and [`ToQuery`] for
//! canonical production. Parsing follows `application/x-www-form-urlencoded`
//! rules: `%XX` escapes are decoded and `+` represents a space.
//!
//! Fields may be scalar values, [`Option`] values, or repeated
//! [`Vec`](alloc::vec::Vec) values.
//! Values other than strings are decoded through [`core::str::FromStr`] and
//! encoded through [`core::fmt::Display`]. The `query` derive attributes
//! support renaming, aliases, defaults, flattened query types, skipped fields,
//! and rejecting unknown parameters. Names claimed by more than one direct or
//! flattened schema are rejected as ambiguous.
//! Common compatible
//! `serde` field and container attributes are also accepted.
//!
//! Parsing and production apply [`QueryLimits`] so untrusted inputs cannot
//! force unbounded work or output. Use [`FromQuery::from_query_with`] or
//! [`ToQuery::to_query_string_with`] or [`ToQuery::write_query_with`] to supply
//! application-specific limits.
//!
//! # Derive helper attributes
//!
//! `#[derive(FromQuery)]` and `#[derive(ToQuery)]` register `#[query(...)]` as a
//! derive helper attribute. It is not a standalone attribute macro, so rustdoc
//! documents its options on the [`FromQuery`](macro@FromQuery) and
//! [`ToQuery`](macro@ToQuery) derive pages rather than creating a separate
//! `query` attribute page.
//!
//! Container attributes control field renaming with `rename_all` and decoding
//! of unknown parameters with `deny_unknown_fields`. Field attributes provide
//! `rename`, repeatable `alias`, `default`, `flatten`, and `skip`. A
//! [`Vec`](alloc::vec::Vec) field always represents a repeated parameter. See
//! the derive pages for complete semantics and restrictions.
//!
//! # Examples
//!
//! ```
//! use routerama::query::{FromQuery, ToQuery};
//!
//! #[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
//! #[query(deny_unknown_fields)]
//! struct SearchQuery {
//!     q: String,
//!     page: Option<usize>,
//!     #[query(rename = "tag")]
//!     tags: Vec<String>,
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let parsed = SearchQuery::from_query("q=rust+language&page=2&tag=fast&tag=safe")?;
//! assert_eq!(parsed.q, "rust language");
//! assert_eq!(parsed.tags, ["fast", "safe"]);
//!
//! let encoded = parsed.to_query_string()?;
//! assert_eq!(encoded, "q=rust+language&page=2&tag=fast&tag=safe");
//! # Ok(())
//! # }
//! ```

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

/// Repeated query-field storage used by generated decoders.
#[doc(hidden)]
pub type Repeated<T> = alloc::vec::Vec<T>;

#[doc(hidden)]
pub use decode_fields::DecodeFields;
#[doc(hidden)]
pub use decoded::{Decoded, parse_borrowed, parse_cow, parse_owned, parse_value};
#[doc(hidden)]
pub use encode_fields::EncodeFields;
#[doc(hidden)]
pub use encoder::Encoder;
pub use error::Error;
pub use error_kind::ErrorKind;
pub use from_query::FromQuery;
#[doc(hidden)]
pub use query_decoder::QueryDecoder;
pub use query_limits::QueryLimits;
pub use routerama_macros::{FromQuery, ToQuery};
pub use to_query::ToQuery;
