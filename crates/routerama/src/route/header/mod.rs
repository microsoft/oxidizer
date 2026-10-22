// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed accessors for selected HTTP header fields.
//!
//! Date parsing is delegated to the [`headers`] crate; `Cache-Control` and
//! `Accept-Encoding` are parsed here, directly over the field bytes.
//! [`HeaderCache`] memoizes expensive, repetitive fields in bounded storage.

mod accept_encoding;
mod cache;
mod cache_control;
pub(super) mod grammar;
mod header_ext;

#[doc(inline)]
pub use accept_encoding::{AcceptEncoding, Encoding};
#[doc(inline)]
pub use cache::HeaderCache;
#[doc(inline)]
pub use header_ext::HeaderExt;
pub use headers::{CacheControl, Date, Expires, IfModifiedSince, IfUnmodifiedSince, LastModified};
