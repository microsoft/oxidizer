// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed HTTP route resolution.
//!
//! Use [`resolver`] on a route enum, then resolve methods and paths through the
//! generated type or the [`Resolver`] trait.
//!
//! # Matching semantics
//!
//! Matching runs on the raw request path, before any percent-decoding or
//! normalization. The rules below are deliberate and apply to every resolver
//! and router in this crate, including mounted services. Each has a security
//! consequence if a caller assumes otherwise, so they are stated here in full.
//! A caller that wants a different rule can canonicalize or refuse a spelling
//! before resolving with [`crate::path::PreparedPath`].
//!
//! ## `%2F` is not a separator
//!
//! Segmentation splits the raw bytes on `/`, so a percent-encoded slash stays
//! inside its segment: `/f/x%2Fy` is two segments and matches `/f/{a}`, not
//! `/f/{a}/{b}`. Literal template parts likewise compare raw bytes, so
//! `/foo/bar` does not match the request `/foo/b%61r`.
//!
//! Decoding happens per capture, during coercion, and therefore after the
//! route has already been chosen. A decoded capture can consequently contain
//! a real separator, a dot segment, or a NUL byte:
//! `/files/..%2F..%2Fetc%2Fpasswd` yields the capture `../../etc/passwd`, and
//! `/id/a%00b` yields `a\0b`. Decoding itself is strict — malformed escapes
//! and invalid UTF-8 are rejected — but a handler that joins a capture into a
//! filesystem path, a URL, or a command must validate the decoded value
//! itself. Matching a route is not a validation step.
//!
//! ## Dot segments are never normalized
//!
//! `/a/../b` has the three segments `a`, `..`, and `b`, and `..` is matched as
//! an ordinary literal or capture value; `%2e%2e` is not decoded before
//! matching either. Nothing collapses `.` or `..` at any point.
//!
//! This matters when a proxy sits in front of the service. If the proxy
//! normalizes dot segments while forwarding the original raw path, the
//! proxy's view of the path and this crate's view differ, and any
//! authorization the proxy performs on the normalized form can be bypassed.
//! Either have the fronting proxy forward the path it actually authorized, or
//! reject dot segments before they reach the resolver — which is what
//! [`crate::path::PreparedPath`] exists to do.
//!
//! ## Trailing slashes and empty segments are significant
//!
//! No two spellings of a path are treated as equivalent. `/a/b/` has three
//! segments and does not match the template `/a/b`. Templates cannot declare
//! an empty segment at all, and `{var}` refuses to match one, so an empty
//! segment can only ever be consumed by a `**` catch-all. Literal segments and
//! methods compare case-sensitively. A leading `/` is optional on the request
//! path, so `resolve("GET", "admin/1")` matches the template `/admin/{id}`.
//!
//! ## A `:verb` route makes verb splitting table-wide
//!
//! Declaring a single route with a trailing `:verb` suffix anywhere in the
//! table enables verb splitting for *every* path the table resolves, because
//! the split happens before the trie is walked. Since `:` is an ordinary path
//! character, a capture value that contains one then becomes unreachable: with
//! a `:archive` route present, `GET /a/b:c` no longer matches `/a/{v}`, since
//! the path is split into the body `/a/b` and the verb `archive`, which no
//! route claims. Verb splitting never selects a *wrong* route — the verb must
//! compare equal at the leaf — so this costs availability rather than
//! integrity, but it means a table that mixes `:verb` routes with captures
//! that may contain a colon needs those captures encoded.

pub use routerama_macros::resolver;

pub use crate::configuration_error::ConfigurationError;
pub use crate::http_method::HttpMethod;
pub use crate::resolve_error::ResolveError;
pub use crate::resolver::Resolver;

/// Runtime support referenced by generated resolvers.
#[doc(hidden)]
pub mod __private {
    pub use routerama_build::Route;

    pub use crate::captures::Captures;
    pub use crate::codegen_helpers::{
        InvalidPath, RouteMatch, ScannedPath, ScannedPathPrefix, scan_path, scan_segments, seg_bytes, split_verb, substr, with_scanned_path,
    };
    pub use crate::configuration_error::ConfigurationError;
    pub use crate::dyn_builder::DynBuilder;
    pub use crate::dyn_route::DynRoute;
    pub use crate::extract_helpers::{
        PrimitiveCapture, PrimitivePath, coerce_cow, coerce_owned, coerce_parse, coerce_primitive, owned, parse, primitive,
    };
    pub use crate::http_method::HttpMethod;
    pub use crate::raw_match::RawMatch;
    pub use crate::raw_resolver::RawResolver;
    pub use crate::resolve_error::ResolveError;
    pub use crate::resolver::Resolver;
}
