// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recipes and patterns for common HTTP workloads.
//!
//! A cookbook of practical examples for using [`http_extensions`](crate) effectively.
//!
//! # Use dedicated types
//!
//! Prefer the purpose-built types over plain `String` for URIs, headers, and
//! bodies. They enforce correctness at construction time and enable
//! zero-allocation patterns that `String` cannot.
//!
//! This is just a specialization of [`M-STRONG-TYPES`](https://microsoft.github.io/rust-guidelines/guidelines/libs/resilience/index.html?highlight=strong#M-STRONG-TYPES)
//! rule for HTTP-related types.
//!
//! ## URI family
//!
//! | Type | Represents | Example |
//! |------|-----------|---------|
//! | [`Origin`] | scheme + authority (host & port) | `https://api.example.com:443` |
//! | [`BasePath`] | a path prefix that starts and ends with `/` | `/v2/` |
//! | [`BaseUri`] | [`Origin`] + [`BasePath`] | `https://api.example.com/v2/` |
//! | [`Uri`] | full request target (optional base + path + query) | `https://api.example.com/v2/users?page=1` |
//!
//! Each level composes into the next, so you can build a [`Uri`] from
//! reusable pieces without string concatenation:
//!
//! ## Header family ([`http::header`])
//!
//! | Type | Represents |
//! |------|-----------|
//! | [`HeaderName`] | a header name — ~90 standard names are interned as zero-cost enum variants |
//! | [`HeaderValue`] | a header value — supports static, borrowed, and owned sources |
//! | [`HeaderMap`] | an optimised multimap of headers with Robin Hood hashing |
//!
//! See [Prefer `HeaderMap` over `HashMap<String, String>`](#prefer-headermap-over-hashmapstring-string)
//! below for details on zero-allocation patterns.
//!
//! ## Body family
//!
//! | Type | Represents |
//! |------|-----------|
//! | [`HttpBody`] | an HTTP body — text, binary, JSON, or streaming content |
//! | [`BytesView`](bytesbuf::BytesView) | a non-contiguous byte buffer with pooled memory and zero-copy slicing |
//!
//! [`HttpBody`] is the body type used throughout `http_extensions` requests and
//! responses. It is created via [`HttpBodyBuilder`] and can be converted
//! into a [`BytesView`](bytesbuf::BytesView) for efficient downstream
//! processing without copying.
//!
//! [`BytesView`](bytesbuf::BytesView) is useful when you need to pass raw
//! data around outside the HTTP layer. It stores non-contiguous chunks
//! backed by a memory pool, so buffers are recycled rather than
//! individually allocated and freed.
//!
//! # Prefer `HeaderMap` over `HashMap<String, String>`
//!
//! [`http::HeaderMap`] is purpose-built for HTTP headers. Unlike
//! `HashMap<String, String>`, it can represent headers with **zero
//! allocations** by interning about 90 standard names as enum variants and
//! wrapping static strings via [`HeaderName::from_static`] /
//! [`HeaderValue::from_static`]. Owned `String` and `Vec<u8>` values
//! are reused without copying through `TryFrom`.
//!
//! ```
//! use http::header::{CONTENT_TYPE, ACCEPT, HeaderMap, HeaderValue};
//!
//! let mut headers = HeaderMap::with_capacity(10); // if you know ahead of time
//! // Standard name constant + static value = 0 allocations
//! headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
//! headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
//! ```
//!
//! # Header Names
//!
//! Prefer using predefined header names from the [`http::header`] module when possible, for example
//! [`http::header::CONTENT_TYPE`]. For custom header names, use [`HeaderName::from_static`] with a
//! static string literal and define them as constants.
//!
//! ```
//! use http::header::HeaderName;
//!
//! const X_CUSTOM_HEADER: HeaderName = HeaderName::from_static("x-custom-header");
//! ```
//!
//! # Header Values
//!
//! Prefer using [`HeaderValue::from_static`] for static values and [`HeaderValueExt::from_shared`] for
//! potentially large values. The latter takes a `bytes::Bytes` value, which can be constructed from
//! multiple sources, giving you the flexibility to share data without copying.
//!
//! When you already have a [`BytesView`](bytesbuf::BytesView) (e.g. from a response body or a
//! memory pool), use [`HeaderValueExt::from_shared`] to convert it into a
//! [`HeaderValue`] without extra allocations.
//!
//! ```
//! use bytes::Bytes;
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use http::header::HeaderValue;
//! use http_extensions::HeaderValueExt;
//!
//! // Static value — zero allocations
//! let value = HeaderValue::from_static("application/json");
//!
//! // From bytes::Bytes — zero allocations for static data
//! let large_value = HeaderValue::from_shared(
//!     Bytes::from_static(b"my large value"),
//! )
//! .unwrap();
//!
//! // From a BytesView — zero-copy when backed by a single contiguous slice
//! # let pool = GlobalPool::new();
//! let view = BytesView::copied_from_slice(b"pooled-header-data", &pool);
//! let pooled_value = HeaderValue::from_shared(view.to_bytes()).unwrap();
//! assert_eq!(pooled_value, "pooled-header-data");
//! ```
//!
//! # Keep URIs Around
//!
//! When you access the same resource repeatedly, parse the [`Uri`] once and
//! reuse it. Passing a `&str` each time re-parses and re-allocates on every
//! call; [`Uri::clone`] is cheap by comparison.
//!
//! **Avoid** parsing the same string on every request:
//!
//! ```
//! use http_extensions::HttpRequestBuilder;
//!
//! # fn example() -> Result<(), http_extensions::HttpError> {
//! for _ in 0..100 {
//!     // Parses and allocates a new Uri each iteration.
//!     let request = HttpRequestBuilder::new_fake()
//!         .get("https://api.example.com/health")
//!         .build()?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! **Prefer** to parse once, clone per request:
//!
//! ```
//! use http_extensions::HttpRequestBuilder;
//! use templated_uri::Uri;
//!
//! # fn example() -> Result<(), http_extensions::HttpError> {
//! let uri: Uri = "https://api.example.com/health".try_into()?;
//! for _ in 0..100 {
//!     let request = HttpRequestBuilder::new_fake()
//!         .get(uri.clone())
//!         .build()?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Use Templated URIs
//!
//! Prefer [`templated`] URI types over raw `format!` strings.
//! Templated URIs are **safer** ([`UriSafeString`] rejects
//! reserved characters), **RFC 6570 compliant**, and **faster** (fewer allocations
//! than `format!`-based construction). They also **enhance telemetry**: the template string is
//! automatically attached to every request so logging and metrics handlers can group
//! traffic by route instead of by unique URL.
//!
//! [`HttpRequestBuilder`] methods like [`get`](HttpRequestBuilder::get),
//! [`post`](HttpRequestBuilder::post), and [`uri`](HttpRequestBuilder::uri) accept
//! any `impl TryInto<Uri>`, which includes `#[templated]` structs out of the box.
//!
//! **Avoid** raw string formatting, which loses the template and bypasses validation:
//!
//! ```
//! use http_extensions::HttpRequestBuilder;
//!
//! # fn example(user_id: &str) -> Result<(), http_extensions::HttpError> {
//! let request = HttpRequestBuilder::new_fake()
//!     .get(format!("https://api.example.com/users/{user_id}/profile"))
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! **Prefer** a templated URI struct that is validated, low-allocation, and telemetry-ready:
//!
//! ```
//! use http_extensions::HttpRequestBuilder;
//! use uuid::Uuid;
//! use templated_uri::templated;
//!
//! #[templated(template = "/users/{user_id}/profile", unredacted)]
//! #[derive(Clone)]
//! struct UserProfilePath {
//!     user_id: Uuid,
//! }
//!
//! # fn example(user_id: Uuid) -> Result<(), http_extensions::HttpError> {
//! let request = HttpRequestBuilder::new_fake()
//!     .get(UserProfilePath { user_id })
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! Templated paths are relative; set a base URI on the client so the final
//! request URL is complete.
//!
//! > **Note:** Real HTTP clients that implement [`RequestHandler`] follow the
//! > same pattern via [`HttpRequestBuilderExt::request_builder`], which returns an
//! > [`HttpRequestBuilder`] already wired up for sending. The builder API
//! > (including `.get()`, `.post()`, `.fetch()`, etc.) is identical.
//!
//! # Authorization Tokens
//!
//! Bearer tokens and JSON Web Tokens are often over 1 KB. Building a [`HeaderValue`]
//! from `&str` on every request copies the entire token each time.
//! **Build once, clone many.** Construct a [`HeaderValue`] via
//! [`HeaderValueExt::from_shared`] and `.clone()` it per request
//! (clone is a ref-count bump, zero copy).
//!
//! **Avoid** combining `format!` and `from_str`, which allocates and copies on every call:
//!
//! ```
//! use http::header::HeaderValue;
//!
//! # let token = "eyJhbGciOi...long_token";
//! // Full copy of the token into a new allocation each time.
//! let value = HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
//! ```
//!
//! **Prefer** to build once from [`Bytes`](bytes::Bytes), then clone cheaply:
//!
//! ```
//! use bytes::Bytes;
//! use http::header::{AUTHORIZATION, HeaderMap, HeaderValue};
//! use http_extensions::HeaderValueExt;
//!
//! // One allocation when the token is acquired.
//! let token = "eyJhbGciOi...long_token";
//! let auth = HeaderValue::from_shared(
//!     Bytes::from(format!("Bearer {token}")),
//! )
//! .unwrap();
//!
//! // Each .clone() is a ref-count bump — no data copied.
//! let mut headers = HeaderMap::new();
//! headers.insert(AUTHORIZATION, auth.clone());
//! ```
//!
//! Rebuild the [`HeaderValue`] only when the token is refreshed.
//!

#[expect(unused_imports, reason = "simplifies the docs")]
use crate::*;
#[expect(unused_imports, reason = "simplifies the docs")]
use http::*;
#[expect(unused_imports, reason = "simplifies the docs")]
use templated_uri::*;
