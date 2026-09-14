// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Well-known HTTP header types.
//!
//! Header families expose descriptors plus borrowed `*View` and owned
//! `*Owned` values, re-exported here for convenience.
//!
//! # Examples
//!
//! ```rust
//! # #[cfg(all(feature = "http", feature = "headers-content-type"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use http_headers::headers::ContentType;
//!
//! let mut headers = http::HeaderMap::new();
//! ContentType::insert(&mut headers, ContentType::json())?;
//! assert!(ContentType::view(&headers)?.is_some());
//! ContentType::remove(&mut headers);
//! assert!(ContentType::owned(&headers)?.is_none());
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "http", feature = "headers-content-type")))]
//! # fn main() {}
//! ```

#[cfg(any(test, feature = "headers-authorization"))]
mod authorization;
#[cfg(any(test, feature = "headers-cache-control"))]
mod cache_control;
#[cfg(any(test, feature = "headers-conditional"))]
mod conditional;
#[cfg(any(test, feature = "headers-content-length"))]
mod content_length;
#[cfg(any(test, feature = "headers-content-type"))]
mod content_type;
#[cfg(any(test, feature = "headers-cors"))]
mod cors;
#[cfg(any(test, feature = "headers-etag"))]
mod etag;
mod extension_value;
#[cfg(any(test, feature = "headers-location"))]
mod location;
#[cfg(any(test, feature = "headers-negotiation"))]
mod negotiation;
#[cfg(any(test, feature = "headers-range"))]
mod range;
#[cfg(any(test, feature = "headers-security"))]
mod security;
#[cfg(any(test, feature = "headers-set-cookie"))]
mod set_cookie;
mod shared;
#[cfg(any(test, feature = "headers-user-agent"))]
mod user_agent;
#[cfg(any(test, feature = "headers-websocket"))]
mod websocket;

#[cfg(any(test, feature = "headers-authorization"))]
#[doc(inline)]
pub use authorization::{Authorization, AuthorizationOwned, AuthorizationView, Basic, BasicCredentials, Bearer};
#[cfg(any(test, feature = "headers-cache-control"))]
#[doc(inline)]
pub use cache_control::{CacheControl, CacheControlBuilder, CacheControlOwned, CacheControlView, CacheDirectiveView};
#[cfg(any(test, feature = "headers-conditional"))]
#[doc(inline)]
pub use conditional::{
    ConditionalTagView, IfMatch, IfMatchOwned, IfMatchView, IfModifiedSince, IfModifiedSinceOwned, IfModifiedSinceView, IfNoneMatch,
    IfNoneMatchOwned, IfNoneMatchView, IfRange, IfRangeOwned, IfRangeValueView, IfRangeView, IfUnmodifiedSince, IfUnmodifiedSinceOwned,
    IfUnmodifiedSinceView, LastModified, LastModifiedOwned, LastModifiedView,
};
#[cfg(any(test, feature = "headers-content-length"))]
#[doc(inline)]
pub use content_length::{ContentLength, ContentLengthOwned};
#[cfg(any(test, feature = "headers-content-type"))]
#[doc(inline)]
pub use content_type::{ContentType, ContentTypeOwned, ContentTypeView, MediaTypeParameterView, MediaTypeParameters};
#[cfg(any(test, feature = "headers-cors"))]
#[doc(inline)]
pub use cors::{
    AccessControlAllowCredentials, AccessControlAllowCredentialsOwned, AccessControlAllowCredentialsView, AccessControlAllowHeaders,
    AccessControlAllowHeadersOwned, AccessControlAllowHeadersView, AccessControlAllowMethods, AccessControlAllowMethodsOwned,
    AccessControlAllowMethodsView, AccessControlAllowOrigin, AccessControlAllowOriginOwned, AccessControlAllowOriginView,
    AccessControlExposeHeaders, AccessControlExposeHeadersOwned, AccessControlExposeHeadersView, AccessControlMaxAge,
    AccessControlMaxAgeOwned, AccessControlRequestHeaders, AccessControlRequestHeadersOwned, AccessControlRequestHeadersView,
    AccessControlRequestMethod, AccessControlRequestMethodOwned, AccessControlRequestMethodView, CorsHeaderNameView, CorsMethodView,
};
#[cfg(any(test, feature = "headers-etag"))]
#[doc(inline)]
pub use etag::{ETag, ETagOwned, ETagView};
#[doc(inline)]
pub use extension_value::ExtensionValue;
#[cfg(any(test, feature = "headers-location"))]
#[doc(inline)]
pub use location::{Location, LocationOwned, LocationView};
#[cfg(any(test, feature = "headers-negotiation"))]
#[doc(inline)]
pub use negotiation::{
    Accept, AcceptEncoding, AcceptEncodingOwned, AcceptEncodingView, AcceptLanguage, AcceptLanguageOwned, AcceptLanguageView, AcceptOwned,
    AcceptView, Allow, AllowOwned, AllowView, Host, HostOwned, HostView, Server, ServerOwned, ServerView, Vary, VaryOwned, VaryView,
};
#[cfg(any(test, feature = "headers-range"))]
#[doc(inline)]
pub use range::{
    AcceptRanges, AcceptRangesOwned, AcceptRangesView, ByteContentRange, ByteRangeSpec, CompleteLength, ContentRange, ContentRangeOwned,
    ContentRangeView, Range, RangeOwned, RangeView,
};
#[cfg(any(test, feature = "headers-security"))]
#[doc(inline)]
pub use security::{
    ContentSecurityPolicy, ContentSecurityPolicyOwned, ContentSecurityPolicyView, HstsDirectiveView, ReferrerPolicy, ReferrerPolicyOwned,
    ReferrerPolicyTokenView, ReferrerPolicyValue, ReferrerPolicyView, StrictTransportSecurity, StrictTransportSecurityBuilder,
    StrictTransportSecurityOwned, StrictTransportSecurityView, XContentTypeOptions, XContentTypeOptionsOwned, XContentTypeOptionsView,
};
#[cfg(any(test, feature = "headers-set-cookie"))]
#[doc(inline)]
pub use set_cookie::{SetCookie, SetCookieOwned, SetCookieView};
#[cfg(any(test, feature = "headers-negotiation", feature = "headers-user-agent"))]
use shared::has_non_ows;
#[cfg(any(
    test,
    feature = "headers-authorization",
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-content-type",
    feature = "headers-etag",
    feature = "headers-location",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-user-agent",
    feature = "headers-websocket",
))]
use shared::invalid_syntax;
#[cfg(any(test, feature = "headers-cache-control", feature = "headers-range"))]
use shared::normalized_comma_value;
#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-websocket",
))]
use shared::trim_ows;
#[cfg(any(
    test,
    feature = "headers-authorization",
    feature = "headers-cors",
    feature = "headers-etag",
    feature = "headers-websocket",
))]
use shared::value_from_bytes;
#[cfg(any(test, feature = "headers-user-agent"))]
#[doc(inline)]
pub use user_agent::{UserAgent, UserAgentOwned, UserAgentView};
#[cfg(any(test, feature = "headers-websocket"))]
#[doc(inline)]
pub use websocket::{
    SecWebSocketAccept, SecWebSocketAcceptOwned, SecWebSocketAcceptView, SecWebSocketExtensions, SecWebSocketExtensionsBuilder,
    SecWebSocketExtensionsOwned, SecWebSocketExtensionsView, SecWebSocketKey, SecWebSocketKeyOwned, SecWebSocketKeyView,
    SecWebSocketProtocol, SecWebSocketProtocolOwned, SecWebSocketProtocolView, SecWebSocketVersion, SecWebSocketVersionOwned,
    WebSocketExtensionParameterView, WebSocketExtensionParameters, WebSocketExtensionView,
};
