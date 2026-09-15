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
//! # #[cfg(feature = "http")]
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
//! # #[cfg(not(feature = "http"))]
//! # fn main() {}
//! ```

mod authorization;
mod cache_control;
mod conditional;
mod content_length;
mod content_type;
mod cors;
mod etag;
mod extension_value;
mod location;
mod negotiation;
mod range;
mod security;
mod set_cookie;
mod shared;
mod user_agent;
mod websocket;

#[doc(inline)]
pub use authorization::{Authorization, AuthorizationOwned, AuthorizationView, Basic, BasicCredentials, Bearer};
#[doc(inline)]
pub use cache_control::{CacheControl, CacheControlBuilder, CacheControlOwned, CacheControlView, CacheDirectiveView};
#[doc(inline)]
pub use conditional::{
    ConditionalTagView, IfMatch, IfMatchOwned, IfMatchView, IfModifiedSince, IfModifiedSinceOwned, IfModifiedSinceView, IfNoneMatch,
    IfNoneMatchOwned, IfNoneMatchView, IfRange, IfRangeOwned, IfRangeValueView, IfRangeView, IfUnmodifiedSince, IfUnmodifiedSinceOwned,
    IfUnmodifiedSinceView, LastModified, LastModifiedOwned, LastModifiedView,
};
#[doc(inline)]
pub use content_length::{ContentLength, ContentLengthOwned};
#[doc(inline)]
pub use content_type::{ContentType, ContentTypeOwned, ContentTypeView, MediaTypeParameterView, MediaTypeParameters};
#[doc(inline)]
pub use cors::{
    AccessControlAllowCredentials, AccessControlAllowCredentialsOwned, AccessControlAllowCredentialsView, AccessControlAllowHeaders,
    AccessControlAllowHeadersOwned, AccessControlAllowHeadersView, AccessControlAllowMethods, AccessControlAllowMethodsOwned,
    AccessControlAllowMethodsView, AccessControlAllowOrigin, AccessControlAllowOriginOwned, AccessControlAllowOriginView,
    AccessControlExposeHeaders, AccessControlExposeHeadersOwned, AccessControlExposeHeadersView, AccessControlMaxAge,
    AccessControlMaxAgeOwned, AccessControlRequestHeaders, AccessControlRequestHeadersOwned, AccessControlRequestHeadersView,
    AccessControlRequestMethod, AccessControlRequestMethodOwned, AccessControlRequestMethodView, CorsHeaderNameView, CorsMethodView,
};
#[doc(inline)]
pub use etag::{ETag, ETagOwned, ETagView};
#[doc(inline)]
pub use extension_value::ExtensionValue;
#[doc(inline)]
pub use location::{Location, LocationOwned, LocationView};
#[doc(inline)]
pub use negotiation::{
    Accept, AcceptEncoding, AcceptEncodingOwned, AcceptEncodingView, AcceptLanguage, AcceptLanguageOwned, AcceptLanguageView, AcceptOwned,
    AcceptView, Allow, AllowOwned, AllowView, Host, HostOwned, HostView, Server, ServerOwned, ServerView, Vary, VaryOwned, VaryView,
};
#[doc(inline)]
pub use range::{
    AcceptRanges, AcceptRangesOwned, AcceptRangesView, ByteContentRange, ByteRangeSpec, CompleteLength, ContentRange, ContentRangeOwned,
    ContentRangeView, Range, RangeOwned, RangeView,
};
#[doc(inline)]
pub use security::{
    ContentSecurityPolicy, ContentSecurityPolicyOwned, ContentSecurityPolicyView, HstsDirectiveView, ReferrerPolicy, ReferrerPolicyOwned,
    ReferrerPolicyTokenView, ReferrerPolicyValue, ReferrerPolicyView, StrictTransportSecurity, StrictTransportSecurityBuilder,
    StrictTransportSecurityOwned, StrictTransportSecurityView, XContentTypeOptions, XContentTypeOptionsOwned, XContentTypeOptionsView,
};
#[doc(inline)]
pub use set_cookie::{SetCookie, SetCookieOwned, SetCookieView};
use shared::{has_non_ows, invalid_syntax, normalized_comma_value, trim_ows, value_from_bytes};
#[doc(inline)]
pub use user_agent::{UserAgent, UserAgentOwned, UserAgentView};
#[doc(inline)]
pub use websocket::{
    SecWebSocketAccept, SecWebSocketAcceptOwned, SecWebSocketAcceptView, SecWebSocketExtensions, SecWebSocketExtensionsBuilder,
    SecWebSocketExtensionsOwned, SecWebSocketExtensionsView, SecWebSocketKey, SecWebSocketKeyOwned, SecWebSocketKeyView,
    SecWebSocketProtocol, SecWebSocketProtocolOwned, SecWebSocketProtocolView, SecWebSocketVersion, SecWebSocketVersionOwned,
    WebSocketExtensionParameterView, WebSocketExtensionParameters, WebSocketExtensionView,
};
