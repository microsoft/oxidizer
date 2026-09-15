// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fluent APIs for creating response headers.

use std::time::Duration;

use crate::headers::{
    Accept, AcceptEncoding, AcceptEncodingOwned, AcceptEncodingView, AcceptLanguage, AcceptLanguageOwned, AcceptLanguageView, AcceptOwned,
    AcceptRanges, AcceptRangesOwned, AcceptRangesView, AcceptView, AccessControlAllowCredentials, AccessControlAllowCredentialsOwned,
    AccessControlAllowCredentialsView, AccessControlAllowHeaders, AccessControlAllowHeadersOwned, AccessControlAllowHeadersView,
    AccessControlAllowMethods, AccessControlAllowMethodsOwned, AccessControlAllowMethodsView, AccessControlAllowOrigin,
    AccessControlAllowOriginOwned, AccessControlAllowOriginView, AccessControlExposeHeaders, AccessControlExposeHeadersOwned,
    AccessControlExposeHeadersView, AccessControlMaxAge, AccessControlMaxAgeOwned, AccessControlRequestHeaders,
    AccessControlRequestHeadersOwned, AccessControlRequestHeadersView, AccessControlRequestMethod, AccessControlRequestMethodOwned,
    AccessControlRequestMethodView, Allow, AllowOwned, AllowView, Authorization, AuthorizationOwned, AuthorizationView, Basic, Bearer,
    CacheControl, CacheControlBuilder, CacheControlOwned, CacheControlView, ContentLength, ContentLengthOwned, ContentRange,
    ContentRangeOwned, ContentRangeView, ContentSecurityPolicy, ContentSecurityPolicyOwned, ContentSecurityPolicyView, ContentType,
    ContentTypeOwned, ContentTypeView, ETag, ETagOwned, ETagView, Host, HostOwned, HostView, IfMatch, IfMatchOwned, IfMatchView,
    IfModifiedSince, IfModifiedSinceOwned, IfModifiedSinceView, IfNoneMatch, IfNoneMatchOwned, IfNoneMatchView, IfRange, IfRangeOwned,
    IfRangeView, IfUnmodifiedSince, IfUnmodifiedSinceOwned, IfUnmodifiedSinceView, LastModified, LastModifiedOwned, LastModifiedView,
    Location, LocationOwned, LocationView, Range, RangeOwned, RangeView, ReferrerPolicy, ReferrerPolicyOwned, ReferrerPolicyView,
    SecWebSocketAccept, SecWebSocketAcceptOwned, SecWebSocketAcceptView, SecWebSocketExtensions, SecWebSocketExtensionsOwned,
    SecWebSocketExtensionsView, SecWebSocketKey, SecWebSocketKeyOwned, SecWebSocketKeyView, SecWebSocketProtocol,
    SecWebSocketProtocolOwned, SecWebSocketProtocolView, SecWebSocketVersion, SecWebSocketVersionOwned, Server, ServerOwned, ServerView,
    SetCookie, SetCookieOwned, SetCookieView, StrictTransportSecurity, StrictTransportSecurityOwned, StrictTransportSecurityView,
    UserAgent, UserAgentOwned, UserAgentView, Vary, VaryOwned, VaryView, XContentTypeOptions, XContentTypeOptionsOwned,
    XContentTypeOptionsView,
};
use crate::sink::{FieldSink, InsertError, U64Encoder, ValueRefsEncoder};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldValueRef};

macro_rules! built_in_fields {
    ($macro:ident) => {
        $macro! {
            (CacheControlOwned, CacheControl),
            (AcceptOwned, Accept),
            (AcceptEncodingOwned, AcceptEncoding),
            (AcceptLanguageOwned, AcceptLanguage),
            (AllowOwned, Allow),
            (HostOwned, Host),
            (ServerOwned, Server),
            (VaryOwned, Vary),
            (AcceptRangesOwned, AcceptRanges),
            (ContentRangeOwned, ContentRange),
            (RangeOwned, Range),
            (ETagOwned, ETag),
            (LocationOwned, Location),
            (UserAgentOwned, UserAgent),
            (ContentTypeOwned, ContentType),
            (IfMatchOwned, IfMatch),
            (IfNoneMatchOwned, IfNoneMatch),
            (IfModifiedSinceOwned, IfModifiedSince),
            (IfUnmodifiedSinceOwned, IfUnmodifiedSince),
            (IfRangeOwned, IfRange),
            (LastModifiedOwned, LastModified),
            (AccessControlAllowCredentialsOwned, AccessControlAllowCredentials),
            (AccessControlAllowHeadersOwned, AccessControlAllowHeaders),
            (AccessControlAllowMethodsOwned, AccessControlAllowMethods),
            (AccessControlAllowOriginOwned, AccessControlAllowOrigin),
            (AccessControlExposeHeadersOwned, AccessControlExposeHeaders),
            (AccessControlMaxAgeOwned, AccessControlMaxAge),
            (AccessControlRequestHeadersOwned, AccessControlRequestHeaders),
            (AccessControlRequestMethodOwned, AccessControlRequestMethod),
            (ContentSecurityPolicyOwned, ContentSecurityPolicy),
            (ReferrerPolicyOwned, ReferrerPolicy),
            (StrictTransportSecurityOwned, StrictTransportSecurity),
            (XContentTypeOptionsOwned, XContentTypeOptions),
            (SecWebSocketAcceptOwned, SecWebSocketAccept),
            (SecWebSocketExtensionsOwned, SecWebSocketExtensions),
            (SecWebSocketKeyOwned, SecWebSocketKey),
            (SecWebSocketProtocolOwned, SecWebSocketProtocol),
            (SecWebSocketVersionOwned, SecWebSocketVersion),
            (AuthorizationOwned<Basic>, Authorization<Basic>),
            (AuthorizationOwned<Bearer>, Authorization<Bearer>),
            (SetCookieOwned, SetCookie),
            (ContentLengthOwned, ContentLength),
        }
    };
}

macro_rules! insert_owned {
    ($(($owned:ty, $header:ty)),+ $(,)?) => {
        $(
            impl $owned {
                /// Replaces every field line for this value's header.
                ///
                /// # Errors
                ///
                /// Returns an error without changing the sink when encoding
                /// or storage fails.
                pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
                where
                    S: FieldSink + ?Sized,
                {
                    <$header as Field>::insert(sink, self)
                }
            }
        )+
    };
}

macro_rules! inherent_field_operations {
    ($(($owned:ty, $header:ty)),+ $(,)?) => {
        $(
            impl $header {
                /// Reads a borrowed typed view from `source`.
                ///
                /// # Errors
                ///
                /// Returns an error when a present field is malformed.
                #[inline]
                pub fn view<S>(
                    source: &S,
                ) -> Result<Option<<Self as Field>::View<'_>>, DecodeError>
                where
                    S: FieldSource + ?Sized,
                {
                    <Self as Field>::view(source)
                }

                /// Reads an independently owned field from `source`.
                ///
                /// # Errors
                ///
                /// Returns an error when a present field is malformed.
                #[inline]
                pub fn owned<S>(source: &S) -> Result<Option<$owned>, DecodeError>
                where
                    S: FieldSource + ?Sized,
                {
                    <Self as Field>::owned(source)
                }

                /// Inserts a field, replacing every existing value with that name.
                ///
                /// # Errors
                ///
                /// Returns an error without changing the sink when it cannot
                /// hold the encoded values.
                #[inline]
                pub fn insert<S>(sink: &mut S, value: $owned) -> Result<(), InsertError>
                where
                    S: FieldSink + ?Sized,
                {
                    <Self as Field>::insert(sink, value)
                }

                /// Removes every field value stored for this field.
                #[inline]
                pub fn remove<S>(sink: &mut S)
                where
                    S: FieldSink + ?Sized,
                {
                    <Self as Field>::remove(sink);
                }
            }
        )+
    };
}

built_in_fields!(insert_owned);
built_in_fields!(inherent_field_operations);

macro_rules! insert_field_view {
    ($(($view:ident, $header:ty, $method:ident)),+ $(,)?) => {
        $(
            impl<'a> $view<'a> {
                /// Replaces every field line for this view's header.
                ///
                /// # Errors
                ///
                /// Returns an error without changing the sink when encoding
                /// or storage fails.
                pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
                where
                    S: FieldSink,
                {
                    let value = self.$method();
                    sink.set_encoded(<$header as Field>::name(), value)
                }
            }
        )+
    };
}

macro_rules! insert_values_view {
    ($(($view:ident, $header:ty, $method:ident)),+ $(,)?) => {
        $(
            impl<'a> $view<'a> {
                /// Replaces every field line for this view's header.
                ///
                /// # Errors
                ///
                /// Returns an error without changing the sink when encoding
                /// or storage fails.
                pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
                where
                    S: FieldSink,
                {
                    sink.set_encoded(
                        <$header as Field>::name(),
                        ValueRefsEncoder::new(self.$method()),
                    )
                }
            }
        )+
    };
}

insert_field_view! {
    (ContentTypeView, ContentType, as_field_value),
    (IfModifiedSinceView, IfModifiedSince, as_field_value),
    (IfUnmodifiedSinceView, IfUnmodifiedSince, as_field_value),
    (LastModifiedView, LastModified, as_field_value),
    (IfRangeView, IfRange, as_field_value),
    (AccessControlAllowCredentialsView, AccessControlAllowCredentials, as_field_value),
    (AccessControlAllowOriginView, AccessControlAllowOrigin, as_field_value),
    (AccessControlRequestMethodView, AccessControlRequestMethod, as_field_value),
    (ContentRangeView, ContentRange, as_field_value),
    (RangeView, Range, as_field_value),
    (StrictTransportSecurityView, StrictTransportSecurity, as_field_value),
    (XContentTypeOptionsView, XContentTypeOptions, as_field_value),
    (SecWebSocketAcceptView, SecWebSocketAccept, as_field_value),
    (SecWebSocketKeyView, SecWebSocketKey, as_field_value),
    (ETagView, ETag, field_value),
    (UserAgentView, UserAgent, field_value),
}

impl<Scheme> AuthorizationView<'_, Scheme> {
    /// Replaces `Authorization` from this borrowed view.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
    where
        S: FieldSink,
        Authorization<Scheme>: Field,
    {
        sink.set_encoded(
            <Authorization<Scheme> as Field>::name(),
            FieldValueRef::new(self.as_field_value().as_bytes()).with_sensitive(true),
        )
    }
}

impl LocationView<'_> {
    /// Replaces `Location` and marks the inserted value as sensitive.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
    where
        S: FieldSink,
    {
        sink.set_encoded(
            <Location as Field>::name(),
            FieldValueRef::new(self.as_bytes()).with_sensitive(true),
        )
    }
}

insert_values_view! {
    (AcceptView, Accept, values),
    (AcceptEncodingView, AcceptEncoding, values),
    (AcceptLanguageView, AcceptLanguage, values),
    (AllowView, Allow, values),
    (VaryView, Vary, values),
    (AccessControlAllowHeadersView, AccessControlAllowHeaders, field_values),
    (AccessControlAllowMethodsView, AccessControlAllowMethods, field_values),
    (AccessControlExposeHeadersView, AccessControlExposeHeaders, field_values),
    (AccessControlRequestHeadersView, AccessControlRequestHeaders, field_values),
    (AcceptRangesView, AcceptRanges, field_values),
    (CacheControlView, CacheControl, field_values),
    (ContentSecurityPolicyView, ContentSecurityPolicy, field_values),
    (ReferrerPolicyView, ReferrerPolicy, field_values),
    (SecWebSocketExtensionsView, SecWebSocketExtensions, field_values),
    (SecWebSocketProtocolView, SecWebSocketProtocol, field_values),
}

insert_field_view! {
    (HostView, Host, as_field_value),
    (ServerView, Server, as_field_value),
}

impl SetCookieView<'_> {
    /// Replaces `Set-Cookie` and marks every inserted field line as sensitive.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
    where
        S: FieldSink,
    {
        sink.set_encoded(
            <SetCookie as Field>::name(),
            ValueRefsEncoder::new(self.iter()).with_sensitive(true),
        )
    }
}

macro_rules! insert_conditional_tags {
    ($(($view:ident, $header:ty)),+ $(,)?) => {
        $(
            impl<'a> $view<'a> {
                /// Replaces every field line for this conditional view.
                ///
                /// # Errors
                ///
                /// Returns an error without changing the sink when storage
                /// fails.
                pub fn insert_into<S>(self, sink: &mut S) -> Result<(), InsertError>
                where
                    S: FieldSink,
                {
                    if self.is_wildcard() {
                        sink.set_encoded(
                            <$header as Field>::name(),
                            FieldValueRef::new(b"*"),
                        )
                    } else {
                        sink.set_encoded(
                            <$header as Field>::name(),
                            ValueRefsEncoder::new(
                                self.tags()
                                    .map(|tag| FieldValueRef::new(tag.as_bytes())),
                            ),
                        )
                    }
                }
            }
        )+
    };
}

insert_conditional_tags! {
    (IfMatchView, IfMatch),
    (IfNoneMatchView, IfNoneMatch),
}

macro_rules! response_methods {
    ($(($method:ident, $header:ty, $owned:ty)),+ $(,)?) => {
        $(
            #[doc = concat!("Replaces `", stringify!($header), "` and continues the fluent chain.")]
            ///
            /// # Errors
            ///
            /// Returns an error without changing the sink when storage fails.
            fn $method(&mut self, value: $owned) -> Result<&mut Self, InsertError> {
                <$header as Field>::insert(self, value)?;
                Ok(self)
            }
        )+
    };
}

/// Fluent methods for creating headers in any [`FieldSink`].
///
/// These write-only conveniences construct response fields. Methods replace
/// the named header unless their name begins with `append_`.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), http_headers::sink::InsertError> {
/// use http_headers::headers::ContentType;
/// use http_headers::sink::FieldSinkExt;
///
/// let mut headers = http::HeaderMap::new();
/// headers
///     .set_content_type(ContentType::json())?
///     .set_content_length(128)?;
/// # Ok::<(), http_headers::sink::InsertError>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub trait FieldSinkExt: FieldSink + Sized {
    response_methods! {
        (set_accept, Accept, AcceptOwned),
        (set_accept_encoding, AcceptEncoding, AcceptEncodingOwned),
        (set_accept_language, AcceptLanguage, AcceptLanguageOwned),
        (set_allow, Allow, AllowOwned),
        (set_host, Host, HostOwned),
        (set_server, Server, ServerOwned),
        (set_vary, Vary, VaryOwned),
        (set_accept_ranges, AcceptRanges, AcceptRangesOwned),
        (set_content_range, ContentRange, ContentRangeOwned),
        (set_range, Range, RangeOwned),
        (set_etag, ETag, ETagOwned),
        (set_location, Location, LocationOwned),
        (set_user_agent, UserAgent, UserAgentOwned),
        (set_content_type, ContentType, ContentTypeOwned),
        (set_if_match, IfMatch, IfMatchOwned),
        (set_if_none_match, IfNoneMatch, IfNoneMatchOwned),
        (set_if_modified_since, IfModifiedSince, IfModifiedSinceOwned),
        (set_if_unmodified_since, IfUnmodifiedSince, IfUnmodifiedSinceOwned),
        (set_if_range, IfRange, IfRangeOwned),
        (set_last_modified, LastModified, LastModifiedOwned),
        (set_access_control_allow_credentials, AccessControlAllowCredentials, AccessControlAllowCredentialsOwned),
        (set_access_control_allow_headers, AccessControlAllowHeaders, AccessControlAllowHeadersOwned),
        (set_access_control_allow_methods, AccessControlAllowMethods, AccessControlAllowMethodsOwned),
        (set_access_control_allow_origin, AccessControlAllowOrigin, AccessControlAllowOriginOwned),
        (set_access_control_expose_headers, AccessControlExposeHeaders, AccessControlExposeHeadersOwned),
        (set_access_control_max_age_value, AccessControlMaxAge, AccessControlMaxAgeOwned),
        (set_access_control_request_headers, AccessControlRequestHeaders, AccessControlRequestHeadersOwned),
        (set_access_control_request_method, AccessControlRequestMethod, AccessControlRequestMethodOwned),
        (set_content_security_policy, ContentSecurityPolicy, ContentSecurityPolicyOwned),
        (set_referrer_policy, ReferrerPolicy, ReferrerPolicyOwned),
        (set_strict_transport_security, StrictTransportSecurity, StrictTransportSecurityOwned),
        (set_x_content_type_options, XContentTypeOptions, XContentTypeOptionsOwned),
        (set_sec_websocket_accept, SecWebSocketAccept, SecWebSocketAcceptOwned),
        (set_sec_websocket_extensions, SecWebSocketExtensions, SecWebSocketExtensionsOwned),
        (set_sec_websocket_key, SecWebSocketKey, SecWebSocketKeyOwned),
        (set_sec_websocket_protocol, SecWebSocketProtocol, SecWebSocketProtocolOwned),
        (set_sec_websocket_version, SecWebSocketVersion, SecWebSocketVersionOwned),
        (set_basic_authorization, Authorization<Basic>, AuthorizationOwned<Basic>),
        (set_bearer_authorization, Authorization<Bearer>, AuthorizationOwned<Bearer>),
        (set_set_cookie, SetCookie, SetCookieOwned),
    }

    /// Replaces `Content-Length` from its semantic integer value.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    fn set_content_length(&mut self, length: u64) -> Result<&mut Self, InsertError> {
        self.set_encoded(<ContentLength as Field>::name(), U64Encoder::new(length))?;
        Ok(self)
    }

    /// Replaces `Cache-Control` from a response cache policy.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    fn set_cache_control(&mut self, plan: CacheControlBuilder) -> Result<&mut Self, InsertError> {
        self.set_encoded(<CacheControl as Field>::name(), plan)?;
        Ok(self)
    }

    /// Replaces `Access-Control-Max-Age` from seconds.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    fn set_access_control_max_age(&mut self, seconds: u64) -> Result<&mut Self, InsertError> {
        self.set_encoded(<AccessControlMaxAge as Field>::name(), U64Encoder::new(seconds))?;
        Ok(self)
    }

    /// Replaces `Access-Control-Max-Age` from a duration.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    fn set_access_control_max_age_duration(&mut self, duration: Duration) -> Result<&mut Self, InsertError> {
        self.set_access_control_max_age(duration.as_secs())
    }

    /// Appends the supplied `Set-Cookie` field lines.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage fails.
    fn append_set_cookie(&mut self, value: SetCookieOwned) -> Result<&mut Self, InsertError> {
        self.append_values(<SetCookie as Field>::name(), value.into_sensitive_encoded_values()?)?;
        Ok(self)
    }
}

impl<S> FieldSinkExt for S where S: FieldSink {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::source::FieldLines;
    use crate::{FieldName, FieldValue, TestSink};

    struct Source<'a> {
        name: &'static FieldName,
        values: &'a [FieldValue],
    }

    impl FieldSource for Source<'_> {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == self.name).then(|| FieldLines::from_slice(name, self.values)).flatten()
        }
    }

    struct RejectSink {
        removed: bool,
    }

    impl FieldSource for RejectSink {
        fn lines(&self, _name: &'static FieldName) -> Option<FieldLines<'_>> {
            None
        }
    }

    impl FieldSink for RejectSink {
        fn set_values(&mut self, _name: &'static FieldName, _values: crate::sink::EncodedValues) -> Result<(), InsertError> {
            Err(InsertError)
        }

        fn append_values(&mut self, _name: &'static FieldName, _values: crate::sink::EncodedValues) -> Result<(), InsertError> {
            Err(InsertError)
        }

        fn remove_values(&mut self, _name: &'static FieldName) {
            self.removed = true;
        }
    }

    macro_rules! exercise_insert {
        ($header:ty, [$($value:literal),+ $(,)?]) => {{
            let values = [$(FieldValue::from_static($value)),+];
            let source = Source {
                name: <$header as Field>::name(),
                values: &values,
            };
            let mut sink = TestSink::new();
            <$header as Field>::owned(&source)
                .expect("fixture is valid")
                .expect("fixture is present")
                .insert_into(&mut sink)
                .expect("owned insertion succeeds");
            <$header as Field>::view(&source)
                .expect("fixture is valid")
                .expect("fixture is present")
                .insert_into(&mut sink)
                .expect("view insertion succeeds");
            sink
        }};
    }

    macro_rules! exercise {
        ($header:ty, $setter:ident, [$($value:literal),+ $(,)?]) => {{
            let values = [$(FieldValue::from_static($value)),+];
            let source = Source {
                name: <$header as Field>::name(),
                values: &values,
            };
            let mut sink = exercise_insert!($header, [$($value),+]);
            sink.$setter(
                <$header as Field>::owned(&source)
                    .expect("fixture is valid")
                    .expect("fixture is present"),
            )
            .expect("fluent insertion succeeds");
            sink
        }};
    }

    #[test]
    fn every_generated_owned_view_and_fluent_path_inserts() {
        drop(exercise!(Accept, set_accept, ["text/html"]));
        drop(exercise!(AcceptEncoding, set_accept_encoding, ["gzip"]));
        drop(exercise!(AcceptLanguage, set_accept_language, ["en-US"]));
        drop(exercise!(Allow, set_allow, ["GET, POST"]));
        drop(exercise!(Host, set_host, ["example.com:443"]));
        drop(exercise!(Server, set_server, ["example/1.0"]));
        drop(exercise!(Vary, set_vary, ["accept-encoding, origin"]));
        drop(exercise!(AcceptRanges, set_accept_ranges, ["bytes"]));
        drop(exercise!(ContentRange, set_content_range, ["bytes 0-1/2"]));
        drop(exercise!(Range, set_range, ["bytes=0-1"]));
        drop(exercise!(ETag, set_etag, ["\"a\""]));
        drop(exercise!(Location, set_location, ["/docs"]));
        drop(exercise!(UserAgent, set_user_agent, ["client/1"]));
        drop(exercise!(ContentType, set_content_type, ["application/json"]));
        drop(exercise!(IfMatch, set_if_match, ["\"a\""]));
        drop(exercise!(IfNoneMatch, set_if_none_match, ["W/\"a\""]));
        drop(exercise!(IfModifiedSince, set_if_modified_since, ["Sun, 06 Nov 1994 08:49:37 GMT"]));
        drop(exercise!(
            IfUnmodifiedSince,
            set_if_unmodified_since,
            ["Sun, 06 Nov 1994 08:49:37 GMT"]
        ));
        drop(exercise!(IfRange, set_if_range, ["\"a\""]));
        drop(exercise!(LastModified, set_last_modified, ["Sun, 06 Nov 1994 08:49:37 GMT"]));
        drop(exercise!(
            AccessControlAllowCredentials,
            set_access_control_allow_credentials,
            ["true"]
        ));
        drop(exercise!(
            AccessControlAllowHeaders,
            set_access_control_allow_headers,
            ["content-type, x-request-id"]
        ));
        drop(exercise!(
            AccessControlAllowMethods,
            set_access_control_allow_methods,
            ["GET, POST"]
        ));
        drop(exercise!(
            AccessControlAllowOrigin,
            set_access_control_allow_origin,
            ["https://example.com"]
        ));
        drop(exercise!(
            AccessControlExposeHeaders,
            set_access_control_expose_headers,
            ["etag, x-request-id"]
        ));
        drop(exercise!(AccessControlMaxAge, set_access_control_max_age_value, ["600"]));
        drop(exercise!(
            AccessControlRequestHeaders,
            set_access_control_request_headers,
            ["content-type, x-request-id"]
        ));
        drop(exercise!(AccessControlRequestMethod, set_access_control_request_method, ["POST"]));
        drop(exercise!(
            ContentSecurityPolicy,
            set_content_security_policy,
            ["default-src 'self'"]
        ));
        drop(exercise!(ReferrerPolicy, set_referrer_policy, ["no-referrer"]));
        drop(exercise!(
            StrictTransportSecurity,
            set_strict_transport_security,
            ["max-age=60; includeSubDomains"]
        ));
        drop(exercise!(XContentTypeOptions, set_x_content_type_options, ["nosniff"]));
        drop(exercise!(
            SecWebSocketAccept,
            set_sec_websocket_accept,
            ["s3pPLMBiTxaQ9kYGzzhZRbK+xOo="]
        ));
        drop(exercise!(
            SecWebSocketExtensions,
            set_sec_websocket_extensions,
            ["permessage-deflate"]
        ));
        drop(exercise!(SecWebSocketKey, set_sec_websocket_key, ["dGhlIHNhbXBsZSBub25jZQ=="]));
        drop(exercise!(SecWebSocketProtocol, set_sec_websocket_protocol, ["chat, superchat"]));
        drop(exercise!(SecWebSocketVersion, set_sec_websocket_version, ["13"]));
        drop(exercise!(Authorization<Basic>, set_basic_authorization, ["Basic Zm9vOmJhcg=="]));
        drop(exercise!(Authorization<Bearer>, set_bearer_authorization, ["Bearer token"]));
        drop(exercise!(SetCookie, set_set_cookie, ["a=1"]));
    }

    #[test]
    fn semantic_and_specialized_response_paths_insert() {
        let mut sink = exercise_insert!(CacheControl, ["private, max-age=60"]);
        sink.set_cache_control(
            CacheControl::public()
                .no_store()
                .must_revalidate()
                .immutable()
                .max_age(Duration::from_secs(u64::MAX))
                .extension_value("x-test", "enabled"),
        )
        .expect("cache plan inserts");
        sink.set_cache_control(CacheControl::private())
            .expect("private plan inserts")
            .set_cache_control(CacheControl::no_cache())
            .expect("no-cache plan inserts")
            .set_content_length(u64::MAX)
            .expect("content length inserts")
            .set_access_control_max_age(600)
            .expect("max age inserts")
            .set_access_control_max_age_duration(Duration::from_mins(5))
            .expect("duration inserts")
            .set_content_type(ContentType::json())
            .expect("JSON content type inserts");

        let mut cookies = SetCookieOwned::new();
        cookies.push_str("a=1").expect("cookie is valid");
        sink.append_set_cookie(cookies).expect("first cookie append succeeds");
        let mut cookies = SetCookieOwned::new();
        cookies.push_str("b=2").expect("cookie is valid");
        sink.append_set_cookie(cookies).expect("second cookie append succeeds");

        ContentLengthOwned::new(42)
            .insert_into(&mut sink)
            .expect("numeric owned insertion succeeds");
    }

    #[test]
    fn specialized_views_cover_wildcards_and_general_storage() {
        drop(exercise_insert!(IfMatch, ["*"]));
        drop(exercise_insert!(IfNoneMatch, ["*"]));
        drop(exercise_insert!(AcceptRanges, ["items"]));
        drop(exercise_insert!(SecWebSocketVersion, ["7, 8, 13"]));
        drop(exercise_insert!(ContentSecurityPolicy, ["default-src 'self'", "img-src https:"]));
        drop(exercise_insert!(ReferrerPolicy, ["no-referrer", "same-origin"]));
        drop(exercise_insert!(SecWebSocketExtensions, ["permessage-deflate", "x-test"]));
        drop(exercise_insert!(SecWebSocketProtocol, ["chat", "superchat"]));
        drop(exercise_insert!(SetCookie, ["a=1", "b=2"]));
    }

    #[test]
    fn borrowed_view_insertion_preserves_source_sensitivity() {
        let values = [FieldValue::from_static("client/1").with_sensitive(true)];
        let source = Source {
            name: &FieldName::UserAgent,
            values: &values,
        };
        let mut sink = TestSink::new();
        UserAgent::view(&source)
            .expect("fixture is valid")
            .expect("fixture is present")
            .insert_into(&mut sink)
            .expect("view insertion succeeds");
        assert!(
            sink.lines(&FieldName::UserAgent)
                .expect("inserted value")
                .exactly_one()
                .expect("one value")
                .is_sensitive()
        );

        let values = [
            FieldValue::from_static("text/html"),
            FieldValue::from_static("application/json").with_sensitive(true),
        ];
        let source = Source {
            name: &FieldName::Accept,
            values: &values,
        };
        Accept::view(&source)
            .expect("fixture is valid")
            .expect("fixture is present")
            .insert_into(&mut sink)
            .expect("view insertion succeeds");
        let inserted: Vec<_> = sink
            .lines(&FieldName::Accept)
            .expect("inserted values")
            .repeated()
            .map(FieldValueRef::is_sensitive)
            .collect();
        assert_eq!(inserted, [false, true]);
    }

    #[test]
    fn empty_cache_plan_reports_insertion_error() {
        let mut sink = TestSink::new();
        assert_eq!(sink.set_cache_control(CacheControlOwned::builder()).err(), Some(InsertError));
    }

    #[test]
    fn fluent_methods_propagate_sink_failures() {
        let mut sink = RejectSink { removed: false };
        assert_eq!(
            sink.set_user_agent(UserAgentOwned::try_from_static("client/1").expect("valid"))
                .err(),
            Some(InsertError)
        );
        assert_eq!(sink.set_content_length(42).err(), Some(InsertError));
        assert_eq!(sink.set_access_control_max_age(600).err(), Some(InsertError));

        let mut cookies = SetCookieOwned::new();
        cookies.push_str("a=1").expect("valid cookie");
        assert_eq!(sink.append_set_cookie(cookies).err(), Some(InsertError));

        sink.remove_values(&FieldName::UserAgent);
        assert!(sink.removed);
    }
}
