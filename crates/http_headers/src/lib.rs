// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_headers/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_headers/favicon.ico")]

//! Efficient and robust HTTP header parsing and creation.
//!
//! This crate provides:
//!
//! - Highly optimized parsing of incoming HTTP headers which produce owned or borrowed
//!   strongly-typed Rust structs. These parsers insulate your code from badly formed
//!   headers.
//!
//! - Highly optimized production of headers, ensuring the headers are well-formed.
//!
//! Header parsing and production are abstracted over their source and destination.
//! The optional `http` feature integrates with
//! [`HeaderMap`](https://docs.rs/http/latest/http/header/struct.HeaderMap.html)
//! plus generic [`Request`](https://docs.rs/http/latest/http/request/struct.Request.html)
//! and [`Response`](https://docs.rs/http/latest/http/response/struct.Response.html)
//! values from the [`http`](https://crates.io/crates/http) crate.
//!
//! # Parsing headers
//!
//! Headers are parsed from an implementation of the [`source::FieldSource`] trait. The `http` crate feature
//! implements this trait for [`HeaderMap`](https://docs.rs/http/latest/http/header/struct.HeaderMap.html),
//! [`Request`](https://docs.rs/http/latest/http/request/struct.Request.html), and
//! [`Response`](https://docs.rs/http/latest/http/response/struct.Response.html).
//! Once you have a source, you can choose to parse into borrowed views or owned structs.
//! Prefer borrowed views when the decoded value does not need to outlive the
//! source as they are generally faster. Use owned structs when the parsed header
//! data needs to be retained (such as in a cache).
//!
//! [`Field::view`] returns a header's
//! borrowed `*View` type, whose lifetime is tied to the source.
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http::HeaderMap;
//! use http_headers::Field;
//! use http_headers::headers::{ContentType, UserAgent};
//!
//! // create a HeaderMap to show how to read from it
//! let mut headers = HeaderMap::new();
//! headers.insert(
//!     http::header::USER_AGENT,
//!     http::HeaderValue::from_static("example-client/1.0"),
//! );
//! headers.insert(
//!     http::header::CONTENT_TYPE,
//!     http::HeaderValue::from_static("application/json; charset=utf-8"),
//! );
//!
//! if let Some(agent) = UserAgent::view(&headers)? {
//!     assert_eq!(agent.as_str()?, "example-client/1.0");
//! }
//!
//! if let Some(content_type) = ContentType::view(&headers)? {
//!     assert_eq!(content_type.type_()?, "application");
//!     assert_eq!(content_type.subtype()?, "json");
//!     assert_eq!(
//!         content_type.parameter("charset")?,
//!         Some(b"utf-8".as_slice())
//!     );
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! Prefer `view` unless the decoded value must outlive the source. Use
//! [`Field::owned`] when you need to retain, move, or independently
//! store the result:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), http_headers::DecodeError> {
//! use http::HeaderMap;
//! use http_headers::Field;
//! use http_headers::headers::UserAgent;
//!
//! let mut headers = HeaderMap::new();
//! headers.insert(
//!     http::header::USER_AGENT,
//!     http::HeaderValue::from_static("example-client/1.0"),
//! );
//!
//! let owned = UserAgent::owned(&headers)?.expect("User-Agent is present");
//! drop(headers);
//! assert_eq!(owned.as_bytes(), b"example-client/1.0");
//! # Ok::<(), http_headers::DecodeError>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! Both methods return `Ok(None)` when the header is absent and `Err` when a
//! present value is malformed.
//!
//! # Producing headers
//!
//! You produce headers by populating an implementation of the [`sink::FieldSink`] trait. The
//! `http` cargo feature implements this trait for
//! [`HeaderMap`](https://docs.rs/http/latest/http/header/struct.HeaderMap.html),
//! [`Request`](https://docs.rs/http/latest/http/request/struct.Request.html), and
//! [`Response`](https://docs.rs/http/latest/http/response/struct.Response.html).
//!
//! The [`sink::FieldSinkExt`] trait provides fluent methods that work for any sink:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), http_headers::sink::InsertError> {
//! use std::time::Duration;
//!
//! use http::HeaderMap;
//! use http_headers::headers::{CacheControl, ContentType};
//! use http_headers::sink::FieldSinkExt;
//!
//! let mut headers = HeaderMap::new();
//! headers
//!     .set_content_type(ContentType::json())?
//!     .set_content_length(1_024)?
//!     .set_cache_control(CacheControl::public().max_age(Duration::from_secs(60)))?;
//! # Ok::<(), http_headers::sink::InsertError>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! An owned value can insert itself when it has already been constructed:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http::HeaderMap;
//! use http_headers::headers::LocationOwned;
//!
//! let mut headers = HeaderMap::new();
//! LocationOwned::try_from("/next")?.insert_into(&mut headers)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! A borrowed view can also be forwarded directly to another sink:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http::HeaderMap;
//! use http_headers::Field;
//! use http_headers::headers::UserAgent;
//!
//! let mut incoming = HeaderMap::new();
//! incoming.insert(
//!     http::header::USER_AGENT,
//!     http::HeaderValue::from_static("example-client/1.0"),
//! );
//!
//! let mut outgoing = HeaderMap::new();
//! if let Some(agent) = UserAgent::view(&incoming)? {
//!     agent.insert_into(&mut outgoing)?;
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! [`Field::insert`] is the generic alternative when the descriptor type is
//! already known. It replaces all existing field lines for that header;
//! [`Field::remove`] removes them instead.
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http::HeaderMap;
//! use http_headers::Field;
//! use http_headers::headers::{UserAgent, UserAgentOwned};
//!
//! let mut headers = HeaderMap::new();
//! UserAgent::insert(
//!     &mut headers,
//!     UserAgentOwned::try_from_static("example-client/1.0")?,
//! )?;
//! UserAgent::remove(&mut headers);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! Repeated field lines remain separate. In particular, `Set-Cookie` values are
//! never comma-joined:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http::HeaderMap;
//! use http_headers::Field;
//! use http_headers::headers::{SetCookie, SetCookieOwned};
//!
//! let mut cookies = SetCookieOwned::new();
//! cookies.push_str("session=abc; Path=/; HttpOnly")?;
//! cookies.push_str("theme=dark; Path=/")?;
//!
//! let mut headers = HeaderMap::new();
//! SetCookie::insert(&mut headers, cookies)?;
//! assert_eq!(headers.get_all(http::header::SET_COOKIE).iter().count(), 2);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! # Serialization
//!
//! The `serde` cargo feature implements Serde serialization and deserialization for every owned header
//! struct, [`FieldName`], [`FieldValue`], and [`sink::EncodedValues`].
//! Headers serialize as an ordered sequence of physical field values and
//! deserialize through relaxed validation, which includes strict syntax and
//! the documented interoperability deviations. This preserves round trips for
//! every owned value produced by the public API:
//!
//! ```rust
//! # #[cfg(all(feature = "serde", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http_headers::headers::UserAgentOwned;
//!
//! let header = UserAgentOwned::try_from("example-client/1.0")?;
//! let json = serde_json::to_string(&header)?;
//! let decoded: UserAgentOwned = serde_json::from_str(&json)?;
//! assert_eq!(decoded.as_bytes(), header.as_bytes());
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "serde", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! Repeated lines retain their boundaries:
//!
//! ```rust
//! # #[cfg(all(feature = "serde", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http_headers::headers::SetCookieOwned;
//!
//! let mut cookies = SetCookieOwned::new();
//! cookies.push_str("session=abc")?;
//! cookies.push_str("theme=dark")?;
//! let json = serde_json::to_string(&cookies)?;
//! let decoded: SetCookieOwned = serde_json::from_str(&json)?;
//! assert_eq!(decoded.len(), 2);
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "serde", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! Serialization is not redaction. Sensitive values include their original
//! bytes and an explicit sensitivity marker, so serialized data must be
//! protected like the header value itself:
//!
//! ```rust
//! # #[cfg(feature = "serde")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http_headers::{FieldSensitivity, FieldValue};
//!
//! let secret =
//!     FieldValue::from_static("credential").with_sensitivity(FieldSensitivity::Sensitive);
//! let json = serde_json::to_string(&secret)?;
//! let decoded: FieldValue = serde_json::from_str(&json)?;
//! assert_eq!(decoded.as_bytes(), b"credential");
//! assert!(decoded.is_sensitive());
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "serde"))]
//! # fn main() {}
//! ```
//!
//! # Strict and relaxed reads
//!
//! [`Field::view`] and [`Field::owned`] use strict syntax. Applications that
//! must accept specific common deviations can request [`DecodeMode::Relaxed`]
//! through [`Field::view_with`] or [`Field::owned_with`]:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http_headers::headers::AcceptEncoding;
//! use http_headers::{DecodeMode, Field};
//!
//! let mut headers = http::HeaderMap::new();
//! headers.insert(
//!     http::header::ACCEPT_ENCODING,
//!     http::HeaderValue::from_static("gzip; q = .5"),
//! );
//!
//! assert!(AcceptEncoding::view(&headers).is_err());
//! assert!(AcceptEncoding::view_with(&headers, DecodeMode::Relaxed)?.is_some());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! Relaxed mode is not a general validation bypass. Each header documents the
//! additional forms it accepts, and the original field bytes are preserved.
//!
//! # Sensitive values
//!
//! Authorization, `Location`, and cookie values are marked sensitive so their
//! `Debug` representations and compatible sinks do not reveal their contents.
//! Basic authentication can be read through a borrowed view while reusing
//! caller-owned decode storage:
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-all"))]
//! # fn main() -> Result<(), http_headers::DecodeError> {
//! use http::HeaderMap;
//! use http_headers::Field;
//! use http_headers::headers::{Authorization, Basic, BasicCredentials};
//!
//! let mut headers = HeaderMap::new();
//! headers.insert(
//!     http::header::AUTHORIZATION,
//!     http::HeaderValue::from_static("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="),
//! );
//!
//! let authorization = Authorization::<Basic>::view(&headers)?.expect("Authorization is present");
//! let mut credentials = BasicCredentials::new();
//! let decoded = authorization.extract(&mut credentials)?;
//! assert_eq!(decoded.username(), b"Aladdin");
//! credentials.clear();
//! # Ok::<(), http_headers::DecodeError>(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-all")))]
//! # fn main() {}
//! ```
//!
//! `BasicCredentials` zeroizes decoded bytes when cleared, reused, or dropped.
//!
//! # Defining a custom single-value header
//!
//! Implement [`SingleValueField`] when a custom header is represented by exactly
//! one field line. The crate then supplies its [`Field`] implementation,
//! including borrowed and owned reads, singleton cardinality checks, insertion,
//! and removal.
//!
//! ```rust
//! use std::sync::LazyLock;
//!
//! use http_headers::{
//!     DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField,
//! };
//!
//! static REQUEST_ID: LazyLock<FieldName> =
//!     LazyLock::new(|| FieldName::from_static("x-request-id"));
//!
//! struct RequestId;
//!
//! #[derive(Clone, Debug, Eq, PartialEq)]
//! struct RequestIdOwned(FieldValue);
//!
//! #[derive(Clone, Copy, Debug, Eq, PartialEq)]
//! struct RequestIdView<'a>(FieldValueRef<'a>);
//!
//! fn is_token(bytes: &[u8]) -> bool {
//!     !bytes.is_empty()
//!         && bytes
//!             .iter()
//!             .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
//! }
//!
//! impl SingleValueField for RequestId {
//!     type View<'a> = RequestIdView<'a>;
//!     type Owned = RequestIdOwned;
//!
//!     fn name() -> &'static FieldName {
//!         &REQUEST_ID
//!     }
//!
//!     fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
//!         if is_token(value.as_bytes()) {
//!             Ok(RequestIdView(value))
//!         } else {
//!             Err(DecodeError::new(&REQUEST_ID, DecodeErrorKind::InvalidToken))
//!         }
//!     }
//!
//!     fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
//!         if is_token(value.as_bytes()) {
//!             Ok(RequestIdOwned(value))
//!         } else {
//!             Err(DecodeError::new(&REQUEST_ID, DecodeErrorKind::InvalidToken))
//!         }
//!     }
//!
//!     fn as_field_value(value: &Self::Owned) -> &FieldValue {
//!         &value.0
//!     }
//!
//!     fn into_field_value(value: Self::Owned) -> FieldValue {
//!         value.0
//!     }
//! }
//! # Ok::<(), DecodeError>(())
//! ```
//!
//! # Performance
//!
//! [`docs/PERF.md`](https://github.com/microsoft/oxidizer/blob/main/crates/http_headers/docs/PERF.md)
//! records comparative measurements against `headers 0.4.1`. Borrowed reads
//! generally avoid allocations; some operations still cost more because this
//! crate validates more grammar or returns richer semantic types.
//!
//! # Cargo features
//!
//! - `headers-all` (enabled by default): all built-in typed header families.
//! - `headers-authorization`, `headers-cache-control`, `headers-conditional`,
//!   `headers-content-length`, `headers-content-type`, `headers-cors`,
//!   `headers-etag`, `headers-location`, `headers-negotiation`, `headers-range`,
//!   `headers-security`, `headers-set-cookie`, `headers-user-agent`, and
//!   `headers-websocket`: individual built-in header families.
//! - `http`: optional adapter for `http::HeaderMap` and the `http` crate's name,
//!   value, and method types.
//! - `serde`: serialization and deserialization for owned headers,
//!   [`FieldName`], [`FieldValue`], and [`sink::EncodedValues`].
//!
//! Disable default features to use only the core source, sink, name, and value
//! APIs, then enable only the header families an application needs.
//!
//! # What about trailers?
//!
//! Although this crate is named `http_headers`, it fully supports trailers as well.
//! The crate doesn't currently expose any trailer-specific structs however, so you
//! would need to define those structs and implement the parsers yourself as implementations
//! of the traits in this crate.
//!
//! # Alternate crates
//!
//! This crate is an alternative to the popular [`headers`](https://crates.io/crates/headers) crate.
//! `http_headers` has the following benefits:
//!
//! - Faster header parsing and production
//! - Supports more headers
//! - Performs more robust validation to avoid downstream surprises
//! - Supports explicit relaxed parsing options to support common malformed headers
//! - Supports serde

#![forbid(unsafe_code)]

mod decode_error;
mod field;
mod field_name;
mod field_value;
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
pub mod headers;
#[cfg(feature = "http")]
mod http_adapter;
#[cfg(feature = "serde")]
mod serde_impls;
pub mod sink;
pub mod source;
#[cfg(test)]
mod test_sink;
mod validate;

#[doc(inline)]
pub use decode_error::{DecodeError, DecodeErrorKind};
#[doc(inline)]
pub use field::{DecodeMode, Field, SingleValueField};
#[doc(inline)]
pub use field_name::{FieldName, InvalidFieldName};
#[doc(inline)]
pub use field_value::{FieldValue, FieldValueRef, InvalidFieldValue};
#[doc(inline)]
pub use sink::field_encoder::FieldSensitivity;
#[cfg(test)]
pub(crate) use test_sink::TestSink;
