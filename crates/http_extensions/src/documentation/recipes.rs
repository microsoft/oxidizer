// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recipes and patterns for common HTTP workloads.
//!
//! This guide gives examples for common uses of [`http_extensions`](crate).
//!
//! # Use dedicated types
//!
//! Use the dedicated URI, header, and body types instead of plain `String`.
//! These types reject invalid values when you construct them.
//! They also support operations that do not allocate memory.
//!
//! This guidance applies the [`M-STRONG-TYPES`](https://microsoft.github.io/rust-guidelines/guidelines/libs/resilience/index.html?highlight=strong#M-STRONG-TYPES)
//! rule to HTTP types.
//!
//! ## URI family
//!
//! | Type | Represents | Example |
//! |------|-----------|---------|
//! | [`Origin`] | A scheme and authority (host and port) | `https://api.example.com:443` |
//! | [`BasePath`] | A path prefix that starts and ends with `/` | `/v2/` |
//! | [`BaseUri`] | An [`Origin`] and a [`BasePath`] | `https://api.example.com/v2/` |
//! | [`Uri`] | A full request target with an optional base, path, and query | `https://api.example.com/v2/users?page=1` |
//!
//! Each type can form part of the next type.
//! You can build a [`Uri`] from reusable parts without joining strings.
//!
//! ## Header family ([`http::header`])
//!
//! | Type | Represents |
//! |------|-----------|
//! | [`HeaderName`] | A header name. About 90 standard names use enum variants without allocation. |
//! | [`HeaderValue`] | A header value from a static, borrowed, or owned source |
//! | [`HeaderMap`] | An optimized multimap of headers that uses Robin Hood hashing |
//!
//! See [Prefer `HeaderMap` over `HashMap<String, String>`](#prefer-headermap-over-hashmapstring-string)
//! for patterns that do not allocate memory.
//!
//! ## Body family
//!
//! | Type | Represents |
//! |------|-----------|
//! | [`HttpBody`] | An HTTP body with text, binary, JSON, or streaming content |
//! | [`BytesView`](bytesbuf::BytesView) | A non-contiguous byte buffer with pooled memory and slices that do not copy data |
//!
//! `http_extensions` uses [`HttpBody`] for request and response bodies.
//! [`HttpBodyBuilder`] creates a body.
//! You can convert the body into a [`BytesView`](bytesbuf::BytesView) without copying its data.
//!
//! Use [`BytesView`](bytesbuf::BytesView) to pass raw data outside the HTTP layer.
//! It stores non-contiguous chunks in a memory pool.
//! The pool reuses each buffer instead of allocating and freeing it separately.
//!
//! # Prefer `HeaderMap` over `HashMap<String, String>`
//!
//! [`http::HeaderMap`] is designed for HTTP headers.
//! Unlike `HashMap<String, String>`, it can represent many headers without allocation.
//! It stores about 90 standard names as enum variants.
//! [`HeaderName::from_static`] and [`HeaderValue::from_static`] wrap static strings.
//! `TryFrom` reuses owned `String` and `Vec<u8>` values without copying them.
//!
//! ```
//! use http::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
//!
//! let mut headers = HeaderMap::with_capacity(10); // Use this when you know the capacity.
//! // A standard name and a static value do not allocate memory.
//! headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
//! headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
//! ```
//!
//! # Header Names
//!
//! Use predefined header names from [`http::header`] when they are available.
//! For example, use [`http::header::CONTENT_TYPE`].
//! For custom names, define constants with [`HeaderName::from_static`] and static string literals.
//!
//! ```
//! use http::header::HeaderName;
//!
//! const X_CUSTOM_HEADER: HeaderName = HeaderName::from_static("x-custom-header");
//! ```
//!
//! # Header Values
//!
//! Use [`HeaderValue::from_static`] for static values.
//! Use [`HeaderValueExt::from_shared`] for values that can be large.
//! It accepts `impl Into<Bytes>` from several source types and shares the data without copying it.
//!
//! If you have a [`BytesView`](bytesbuf::BytesView), convert it with
//! [`HeaderValueExt::from_shared`].
//! For example, the view can come from a response body or a memory pool.
//!
//! ```
//! use bytes::Bytes;
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use http::header::HeaderValue;
//! use http_extensions::HeaderValueExt;
//!
//! // This static value does not allocate memory.
//! let value = HeaderValue::from_static("application/json");
//!
//! // Static data in bytes::Bytes does not allocate memory.
//! let large_value = HeaderValue::from_shared(Bytes::from_static(b"my large value")).unwrap();
//!
//! // A single contiguous BytesView slice does not copy data.
//! # let pool = GlobalPool::new();
//! let view = BytesView::copied_from_slice(b"pooled-header-data", &pool);
//! let pooled_value = HeaderValue::from_shared(view.to_bytes()).unwrap();
//! assert_eq!(pooled_value, "pooled-header-data");
//! ```
//!
//! # Keep URIs Around
//!
//! If you access the same resource repeatedly, parse the [`Uri`] once and reuse it.
//! A `&str` causes parsing and allocation on every call.
//! [`Uri::clone`] costs less.
//!
//! Do not parse the same string for every request:
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # (|| {
//! use http_extensions::HttpRequestBuilder;
//!
//! for _ in 0..100 {
//!     // Parses and allocates a new Uri each iteration.
//!     let request = HttpRequestBuilder::new_fake()
//!         .get("https://api.example.com/health")
//!         .build()?;
//! }
//! # Ok::<(), http_extensions::HttpError>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! Parse once and clone the value for each request:
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # (|| {
//! use http_extensions::HttpRequestBuilder;
//! use templated_uri::Uri;
//!
//! let uri: Uri = "https://api.example.com/health".try_into()?;
//! for _ in 0..100 {
//!     let request = HttpRequestBuilder::new_fake().get(uri.clone()).build()?;
//! }
//! # Ok::<(), http_extensions::HttpError>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! # Use Templated URIs
//!
//! Use [`templated`] URI types instead of raw `format!` strings.
//! [`EscapedString`] rejects reserved characters.
//! Templated URIs follow RFC 6570.
//! They allocate less memory than construction with `format!`.
//! Each request stores the template string.
//! Logging and metrics handlers can then group traffic by route instead of by unique URL.
//!
//! [`HttpRequestBuilder`] methods like [`get`](HttpRequestBuilder::get),
//! [`post`](HttpRequestBuilder::post), and [`uri`](HttpRequestBuilder::uri) accept
//! any `impl TryInto<Uri>`.
//! This includes `#[templated]` structs.
//!
//! Do not use raw string formatting.
//! It loses the template and does not reject invalid URI values during construction.
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # (|| {
//! use http_extensions::HttpRequestBuilder;
//!
//! # let user_id = "test-user";
//! let request = HttpRequestBuilder::new_fake()
//!     .get(format!("https://api.example.com/users/{user_id}/profile"))
//!     .build()?;
//! # Ok::<(), http_extensions::HttpError>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! Use a templated URI struct.
//! It rejects invalid values, uses few allocations, and provides the template for telemetry.
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # (|| {
//! use http_extensions::HttpRequestBuilder;
//! use templated_uri::templated;
//! use uuid::Uuid;
//!
//! #[templated(template = "/users/{user_id}/profile", unredacted)]
//! #[derive(Clone)]
//! struct UserProfilePath {
//!     user_id: Uuid,
//! }
//!
//! # let user_id = Uuid::nil();
//! let request = HttpRequestBuilder::new_fake()
//!     .get(UserProfilePath { user_id })
//!     .build()?;
//! # Ok::<(), http_extensions::HttpError>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! Templated paths are relative.
//! Set a base URI on the client to create the complete request URL.
//!
//! > [!NOTE]
//! > HTTP clients that implement [`RequestHandler`] use the same pattern.
//! > [`HttpRequestBuilderExt::request_builder`] returns a builder that can send requests.
//! > The builder provides methods such as `.get()`, `.post()`, and `.fetch()`.
//!
//! # Authorization Tokens
//!
//! Bearer tokens and JSON Web Tokens are often over 1 KB. Building a [`HeaderValue`]
//! from `&str` copies the complete token for every request.
//! Build the value once with [`HeaderValueExt::from_shared`].
//! Clone it for each request.
//! A clone increments a reference count and does not copy the token data.
//!
//! Do not combine `format!` and `from_str`.
//! This combination allocates and copies memory for every call.
//!
//! ```
//! use http::header::HeaderValue;
//!
//! # let token = "eyJhbGciOi...long_token";
//! // Full copy of the token into a new allocation each time.
//! let value = HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
//! ```
//!
//! Build the value once from [`Bytes`](bytes::Bytes), then clone it:
//!
//! ```
//! use bytes::Bytes;
//! use http::header::{AUTHORIZATION, HeaderMap, HeaderValue};
//! use http_extensions::HeaderValueExt;
//!
//! // One allocation when the token is acquired.
//! let token = "eyJhbGciOi...long_token";
//! let auth = HeaderValue::from_shared(Bytes::from(format!("Bearer {token}"))).unwrap();
//!
//! // Each .clone() increments a reference count and does not copy data.
//! let mut headers = HeaderMap::new();
//! headers.insert(AUTHORIZATION, auth.clone());
//! ```
//!
//! Rebuild the [`HeaderValue`] only when the token is refreshed.

#[expect(unused_imports, reason = "keeps the examples concise")]
use ::http::*;
#[expect(unused_imports, reason = "keeps the examples concise")]
use templated_uri::*;

#[expect(unused_imports, reason = "keeps the examples concise")]
use crate::*;
