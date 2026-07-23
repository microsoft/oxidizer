// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed HTTP route resolution.
//!
//! Use [`resolver`] on a route enum, then resolve methods and paths through the
//! generated type or the [`Resolver`] trait.

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
        InvalidPath, RouteMatch, ScannedPath, scan_path, scan_segments, seg_bytes, split_verb, substr, with_scanned_path,
    };
    pub use crate::configuration_error::ConfigurationError;
    pub use crate::dyn_builder::DynBuilder;
    pub use crate::dyn_route::DynRoute;
    pub use crate::extract_helpers::{coerce_cow, coerce_owned, coerce_parse, owned, parse};
    pub use crate::http_method::HttpMethod;
    pub use crate::raw_match::RawMatch;
    pub use crate::raw_resolver::RawResolver;
    pub use crate::resolve_error::ResolveError;
    pub use crate::resolver::Resolver;
}
