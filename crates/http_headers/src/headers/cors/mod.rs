// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-Origin Resource Sharing request and response headers.

mod access_control_allow_credentials;
mod access_control_allow_headers;
mod access_control_allow_methods;
mod access_control_allow_origin;
mod access_control_expose_headers;
mod access_control_max_age;
mod access_control_request_headers;
mod access_control_request_method;
mod cors_header_names;
mod cors_methods;
mod cors_tokens;
mod shared;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_map;

#[doc(inline)]
pub use access_control_allow_credentials::{
    AccessControlAllowCredentials, AccessControlAllowCredentialsOwned, AccessControlAllowCredentialsView,
};
#[doc(inline)]
pub use access_control_allow_headers::{AccessControlAllowHeaders, AccessControlAllowHeadersOwned, AccessControlAllowHeadersView};
#[doc(inline)]
pub use access_control_allow_methods::{AccessControlAllowMethods, AccessControlAllowMethodsOwned, AccessControlAllowMethodsView};
#[doc(inline)]
pub use access_control_allow_origin::{
    AccessControlAllowOrigin, AccessControlAllowOriginKind, AccessControlAllowOriginOwned, AccessControlAllowOriginView, OriginDomainView,
    OriginHost, OriginScheme, SerializedOriginView,
};
#[doc(inline)]
pub use access_control_expose_headers::{AccessControlExposeHeaders, AccessControlExposeHeadersOwned, AccessControlExposeHeadersView};
#[doc(inline)]
pub use access_control_max_age::{AccessControlMaxAge, AccessControlMaxAgeOwned};
#[doc(inline)]
pub use access_control_request_headers::{AccessControlRequestHeaders, AccessControlRequestHeadersOwned, AccessControlRequestHeadersView};
#[doc(inline)]
pub use access_control_request_method::{AccessControlRequestMethod, AccessControlRequestMethodOwned, AccessControlRequestMethodView};
#[doc(inline)]
pub use cors_header_names::CorsHeaderNames;
#[doc(inline)]
pub use cors_methods::CorsMethods;
