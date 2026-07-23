// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned as _;
use syn::{Attribute, Ident, LitInt, LitStr, Token};

use super::predicate_value::{is_concrete_media_type, is_http_authority};
use crate::is_http_token;

/// The method/path or configured-dynamic portion of a `#[route]` attribute.
#[cfg(any(feature = "resolve", feature = "route"))]
#[derive(Clone)]
pub(super) enum RouteTarget {
    Static { method: String, path: LitStr },
    Dynamic,
}

/// An explicitly declared static-route priority.
#[derive(Clone)]
#[cfg_attr(
    not(feature = "route"),
    expect(dead_code, reason = "direct resolvers reject priority by span before its value is used")
)]
#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) struct RoutePriority {
    pub(super) value: i32,
    pub(super) span: proc_macro2::Span,
}

/// Compile-time HTTP request predicates attached to one route declaration.
#[derive(Clone, Default)]
#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) struct RoutePredicates {
    pub(super) host: Option<LitStr>,
    pub(super) consumes: Option<LitStr>,
    pub(super) produces: Option<LitStr>,
}

#[cfg(any(feature = "resolve", feature = "route"))]
impl RoutePredicates {
    #[cfg(feature = "route")]
    pub(super) const fn is_empty(&self) -> bool {
        self.host.is_none() && self.consumes.is_none() && self.produces.is_none()
    }

    #[cfg(feature = "route")]
    pub(super) fn same_values(&self, other: &Self) -> bool {
        same_literal(self.host.as_ref(), other.host.as_ref())
            && same_literal(self.consumes.as_ref(), other.consumes.as_ref())
            && same_literal(self.produces.as_ref(), other.produces.as_ref())
    }

    #[cfg(feature = "route")]
    pub(super) fn differing_literal<'a>(&'a self, other: &Self) -> Option<&'a LitStr> {
        [
            (self.host.as_ref(), other.host.as_ref()),
            (self.consumes.as_ref(), other.consumes.as_ref()),
            (self.produces.as_ref(), other.produces.as_ref()),
        ]
        .into_iter()
        .find_map(|(current, expected)| if same_literal(current, expected) { None } else { current })
    }

    #[cfg(feature = "resolve")]
    pub(super) fn first(&self) -> Option<(&'static str, &LitStr)> {
        self.host
            .as_ref()
            .map(|value| ("host", value))
            .or_else(|| self.consumes.as_ref().map(|value| ("consumes", value)))
            .or_else(|| self.produces.as_ref().map(|value| ("produces", value)))
    }
}

#[cfg(any(feature = "resolve", feature = "route"))]
#[cfg(feature = "route")]
fn same_literal(left: Option<&LitStr>, right: Option<&LitStr>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.value() == right.value(),
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

/// One compile-time response-header operation attached to a route.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) enum StaticHeaderOperation {
    Insert,
    Append,
}

/// One validated compile-time response header attached to a route.
#[derive(Clone)]
#[cfg_attr(
    not(feature = "route"),
    expect(
        dead_code,
        reason = "direct resolvers reject static response headers by name span before using their operation or value"
    )
)]
#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) struct StaticHeader {
    pub(super) operation: StaticHeaderOperation,
    pub(super) name: LitStr,
    pub(super) value: LitStr,
}

#[cfg(feature = "route")]
pub(super) fn same_static_headers(left: &[StaticHeader], right: &[StaticHeader]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.operation == right.operation && left.name.value() == right.name.value() && left.value.value() == right.value.value()
        })
}

#[cfg(feature = "route")]
pub(super) fn differing_static_header<'a>(current: &'a [StaticHeader], expected: &[StaticHeader]) -> Option<&'a LitStr> {
    current
        .iter()
        .zip(expected)
        .find_map(|(current, expected)| {
            if current.operation == expected.operation && current.name.value() == expected.name.value() {
                (current.value.value() != expected.value.value()).then_some(&current.value)
            } else {
                Some(&current.name)
            }
        })
        .or_else(|| (current.len() > expected.len()).then(|| &current[expected.len()].name))
}

/// A parsed `#[route(METHOD, "path", ...)]` or `#[route(dynamic, ...)]`.
///
/// Identifier methods are normalized to uppercase; string methods are used
/// exactly as written and allow any RFC 9110 token.
#[cfg(any(feature = "resolve", feature = "route"))]
#[derive(Clone)]
pub(super) struct RouteAttr {
    pub(super) target: RouteTarget,
    pub(super) predicates: RoutePredicates,
    pub(super) priority: Option<RoutePriority>,
    pub(super) static_headers: Vec<StaticHeader>,
}

#[cfg(any(feature = "resolve", feature = "route"))]
impl Parse for RouteAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let target = if input.peek(Ident) {
            let fork = input.fork();
            let candidate: Ident = fork.parse()?;
            if candidate == "dynamic" {
                let _dynamic: Ident = input.parse()?;
                RouteTarget::Dynamic
            } else {
                parse_static_target(input)?
            }
        } else {
            parse_static_target(input)?
        };
        let (predicates, priority, static_headers) = parse_route_options(input)?;
        Ok(Self {
            target,
            predicates,
            priority,
            static_headers,
        })
    }
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn parse_static_target(input: ParseStream<'_>) -> syn::Result<RouteTarget> {
    let method = if input.peek(LitStr) {
        let method: LitStr = input.parse()?;
        let value = method.value();
        if !is_http_token(&value) {
            return Err(syn::Error::new(
                method.span(),
                "HTTP method strings must be non-empty RFC 9110 `token` values",
            ));
        }
        value
    } else {
        let method: Ident = input.parse()?;
        let value = method_token(&method);
        if !is_http_token(&value) {
            return Err(syn::Error::new(
                method.span(),
                "HTTP method identifiers must produce non-empty RFC 9110 `token` values",
            ));
        }
        value
    };
    let _comma: Token![,] = input.parse()?;
    let path: LitStr = input.parse()?;
    Ok(RouteTarget::Static { method, path })
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn parse_route_options(input: ParseStream<'_>) -> syn::Result<(RoutePredicates, Option<RoutePriority>, Vec<StaticHeader>)> {
    let mut predicates = RoutePredicates::default();
    let mut priority = None;
    let mut static_headers = None;
    if input.is_empty() {
        return Ok((predicates, priority, Vec::new()));
    }

    let _comma: Token![,] = input.parse()?;
    while !input.is_empty() {
        let key: Ident = input
            .parse()
            .map_err(|_error| input.error("expected route argument `host`, `consumes`, `produces`, `priority`, or `headers(...)`"))?;
        if key == "headers" {
            if static_headers.is_some() {
                return Err(syn::Error::new(key.span(), "duplicate `headers(...)` route argument"));
            }
            static_headers = Some(parse_static_headers(input)?);
        } else {
            let _equals: Token![=] = input.parse()?;
            if key == "priority" {
                if priority.is_some() {
                    return Err(syn::Error::new(key.span(), "duplicate `priority` route argument"));
                }
                priority = Some(parse_priority(input)?);
            } else {
                let value: LitStr = input
                    .parse()
                    .map_err(|_error| syn::Error::new(key.span(), format!("`{key}` must be followed by a string literal")))?;
                let slot = match key.to_string().as_str() {
                    "host" => {
                        if !is_http_authority(&value.value()) {
                            return Err(syn::Error::new(
                                value.span(),
                                "`host` must be a legal HTTP authority without a scheme, path, userinfo, whitespace, or an empty host",
                            ));
                        }
                        &mut predicates.host
                    }
                    "consumes" => {
                        if !is_concrete_media_type(&value.value()) {
                            return Err(syn::Error::new(
                                value.span(),
                                "`consumes` must be a concrete `type/subtype` media type without wildcards or parameters",
                            ));
                        }
                        &mut predicates.consumes
                    }
                    "produces" => {
                        if !is_concrete_media_type(&value.value()) {
                            return Err(syn::Error::new(
                                value.span(),
                                "`produces` must be a concrete `type/subtype` media type without wildcards or parameters",
                            ));
                        }
                        &mut predicates.produces
                    }
                    _ => {
                        return Err(syn::Error::new(
                            key.span(),
                            "unknown route argument; expected `host`, `consumes`, `produces`, `priority`, or `headers(...)`",
                        ));
                    }
                };
                if slot.replace(value).is_some() {
                    return Err(syn::Error::new(key.span(), format!("duplicate `{key}` route argument")));
                }
            }
        }
        if input.is_empty() {
            break;
        }
        let _comma: Token![,] = input.parse()?;
        if input.is_empty() {
            break;
        }
    }
    Ok((predicates, priority, static_headers.unwrap_or_default()))
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn parse_static_headers(input: ParseStream<'_>) -> syn::Result<Vec<StaticHeader>> {
    let content;
    syn::parenthesized!(content in input);
    let entries = content.parse_terminated(StaticHeader::parse, Token![,])?;
    Ok(entries.into_iter().collect())
}

#[cfg(any(feature = "resolve", feature = "route"))]
impl Parse for StaticHeader {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let operation: Ident = input
            .parse()
            .map_err(|_error| input.error("expected `insert(\"name\", \"value\")` or `append(\"name\", \"value\")`"))?;
        let operation = match operation.to_string().as_str() {
            "insert" => StaticHeaderOperation::Insert,
            "append" => StaticHeaderOperation::Append,
            _ => {
                return Err(syn::Error::new(
                    operation.span(),
                    "static response-header operations are `insert` or `append`",
                ));
            }
        };
        let content;
        syn::parenthesized!(content in input);
        let name: LitStr = content
            .parse()
            .map_err(|_error| content.error("a static response-header name must be a string literal"))?;
        let _comma: Token![,] = content.parse()?;
        let value: LitStr = content
            .parse()
            .map_err(|_error| content.error("a static response-header value must be a string literal"))?;
        if !content.is_empty() {
            return Err(content.error("a static response-header operation takes exactly one name and one value"));
        }

        let name_value = name.value();
        if name_value.len() > 65_535 || !is_http_token(&name_value) {
            return Err(syn::Error::new(
                name.span(),
                "static response-header names must be non-empty RFC 9110 `token` values",
            ));
        }
        let normalized_name = LitStr::new(&name_value.to_ascii_lowercase(), name.span());
        let value_value = value.value();
        if !value_value.bytes().all(|byte| byte == b'\t' || (32..127).contains(&byte)) {
            return Err(syn::Error::new(
                value.span(),
                "static response-header values must contain only visible ASCII characters or horizontal tabs",
            ));
        }

        Ok(Self {
            operation,
            name: normalized_name,
            value,
        })
    }
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn parse_priority(input: ParseStream<'_>) -> syn::Result<RoutePriority> {
    let negative = input.parse::<Option<Token![-]>>()?.is_some();
    if !negative {
        let _positive = input.parse::<Option<Token![+]>>()?;
    }
    if !input.peek(LitInt) {
        let unexpected: proc_macro2::TokenTree = input
            .parse()
            .map_err(|_error| input.error("`priority` must be an integer in the `i32` range"))?;
        return Err(syn::Error::new(
            unexpected.span(),
            "`priority` must be an integer in the `i32` range",
        ));
    }
    let literal: LitInt = input
        .parse()
        .map_err(|_error| input.error("`priority` must be an integer in the `i32` range"))?;
    if !literal.suffix().is_empty() {
        return Err(syn::Error::new(literal.span(), "`priority` must not have an integer type suffix"));
    }
    let magnitude = literal
        .base10_parse::<i64>()
        .map_err(|_error| syn::Error::new(literal.span(), "`priority` must be an integer in the `i32` range"))?;
    let signed = if negative { magnitude.checked_neg() } else { Some(magnitude) }
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| syn::Error::new(literal.span(), "`priority` must be in the `i32` range"))?;
    Ok(RoutePriority {
        value: signed,
        span: literal.span(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) enum RouteDeclaration {
    Static,
    Dynamic,
}

#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) fn route_declaration(attributes: &[Attribute]) -> syn::Result<Option<RouteDeclaration>> {
    let route_attrs: Vec<_> = attributes.iter().filter(|attribute| attribute.path().is_ident("route")).collect();
    if route_attrs.is_empty() {
        return Ok(None);
    }
    let parsed: Vec<RouteAttr> = route_attrs
        .iter()
        .map(|attribute| attribute.parse_args())
        .collect::<syn::Result<_>>()?;
    if let Some((index, _)) = parsed
        .iter()
        .enumerate()
        .find(|(_, attribute)| matches!(&attribute.target, RouteTarget::Dynamic))
    {
        if route_attrs.len() != 1 {
            return Err(syn::Error::new(
                route_attrs[index].span(),
                "`#[route(dynamic)]` cannot be combined with another route attribute",
            ));
        }
        Ok(Some(RouteDeclaration::Dynamic))
    } else {
        Ok(Some(RouteDeclaration::Static))
    }
}
/// Maps a method identifier to its upper-cased HTTP method token (e.g. `get` →
/// `"GET"`, `HEAD` → `"HEAD"`), which a route matches on case-sensitively.
#[cfg(any(feature = "resolve", feature = "route"))]
fn method_token(ident: &Ident) -> String {
    let spelling = ident.to_string();
    spelling.strip_prefix("r#").unwrap_or(&spelling).to_ascii_uppercase()
}

#[cfg(all(test, any(feature = "resolve", feature = "route")))]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    fn a_valid_route_attribute_parses() {
        // The success path of `RouteAttr::parse` (`METHOD, "path"`).
        let attr = quote! { GET, "/books/{book}" };
        let parsed: RouteAttr = syn::parse2(attr).expect("a `METHOD, \"path\"` attribute parses");
        let RouteTarget::Static { method, path } = parsed.target else {
            panic!("the method-and-path form is static");
        };
        assert_eq!(method, "GET");
        assert_eq!(path.value(), "/books/{book}");
        assert!(parsed.predicates.is_empty());
    }

    #[test]
    fn a_hyphenated_method_string_parses_exactly() {
        let parsed: RouteAttr = syn::parse2(quote! { "M-SEARCH", "/devices" }).expect("a valid HTTP token parses");
        let RouteTarget::Static { method, .. } = parsed.target else {
            panic!("the string method form is static");
        };
        assert_eq!(method, "M-SEARCH");

        let error = syn::parse2::<RouteAttr>(quote! { "BAD METHOD", "/devices" })
            .err()
            .expect("spaces are not allowed in an HTTP token");
        assert!(error.to_string().contains("RFC 9110"), "{error}");
    }

    #[test]
    fn identifier_methods_are_normalized_and_validated() {
        let parsed: RouteAttr = syn::parse2(quote! { r#match, "/items" }).expect("raw method identifier parses");
        let RouteTarget::Static { method, .. } = parsed.target else {
            panic!("the raw method form is static");
        };
        assert_eq!(method, "MATCH");

        let non_token = Ident::new("\u{03bb}", proc_macro2::Span::call_site());
        let error = syn::parse2::<RouteAttr>(quote! { #non_token, "/items" })
            .err()
            .expect("non-token identifier methods are rejected");
        assert!(error.to_string().contains("RFC 9110"), "{error}");
    }

    #[test]
    fn static_and_dynamic_route_predicates_parse_in_any_order() {
        let parsed: RouteAttr = syn::parse2(quote! {
            POST,
            "/items",
            produces = "application/json",
            priority = -12,
            host = "api.example:443",
            headers(
                insert("X-Service", "routerama"),
                append("set-cookie", "one=1"),
            ),
            consumes = "application/json",
        })
        .expect("all static predicates parse");
        assert!(matches!(parsed.target, RouteTarget::Static { .. }));
        assert_eq!(parsed.predicates.host.expect("host").value(), "api.example:443");
        assert_eq!(parsed.predicates.consumes.expect("consumes").value(), "application/json");
        assert_eq!(parsed.predicates.produces.expect("produces").value(), "application/json");
        assert_eq!(parsed.priority.expect("priority").value, -12);
        assert_eq!(parsed.static_headers.len(), 2);
        assert_eq!(parsed.static_headers[0].name.value(), "x-service");
        assert_eq!(parsed.static_headers[0].value.value(), "routerama");
        assert!(matches!(parsed.static_headers[0].operation, StaticHeaderOperation::Insert));
        assert!(matches!(parsed.static_headers[1].operation, StaticHeaderOperation::Append));

        let parsed: RouteAttr = syn::parse2(quote! {
            dynamic,
            consumes = "application/json",
            host = "[2001:db8::1]:8443",
        })
        .expect("configured dynamic predicates parse");
        assert!(matches!(parsed.target, RouteTarget::Dynamic));
        assert_eq!(parsed.predicates.host.expect("host").value(), "[2001:db8::1]:8443");
        assert!(parsed.priority.is_none());
    }

    #[test]
    fn malformed_authorities_and_media_types_are_rejected() {
        for invalid in [
            quote! { GET, "/", host = "" },
            quote! { GET, "/", host = "https://api.example" },
            quote! { GET, "/", host = "user@api.example" },
            quote! { GET, "/", host = "[not-ipv6]" },
            quote! { GET, "/", host = "api.example:65536" },
        ] {
            let error = syn::parse2::<RouteAttr>(invalid).err().expect("malformed authorities are rejected");
            assert!(error.to_string().contains("legal HTTP authority"), "{error}");
        }
        for invalid in [
            quote! { GET, "/", consumes = "application" },
            quote! { GET, "/", consumes = "*/json" },
            quote! { GET, "/", produces = "application/*" },
            quote! { GET, "/", produces = "application/json; charset=utf-8" },
        ] {
            let error = syn::parse2::<RouteAttr>(invalid)
                .err()
                .expect("non-concrete media types are rejected");
            assert!(error.to_string().contains("concrete `type/subtype`"), "{error}");
        }
    }

    #[test]
    fn malformed_static_response_headers_are_rejected_during_route_parsing() {
        for (invalid, message) in [
            (quote! { GET, "/", headers(insert("bad name", "value")) }, "RFC 9110 `token`"),
            (quote! { GET, "/", headers(insert("x-valid", "line\nbreak")) }, "visible ASCII"),
            (quote! { GET, "/", headers(replace("x-valid", "value")) }, "`insert` or `append`"),
            (
                quote! { GET, "/", headers(insert("x-valid", "value", "extra")) },
                "exactly one name and one value",
            ),
        ] {
            let error = syn::parse2::<RouteAttr>(invalid)
                .err()
                .expect("invalid static response headers are rejected");
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[test]
    fn duplicate_unknown_and_malformed_route_arguments_are_rejected() {
        let duplicate = syn::parse2::<RouteAttr>(quote! {
            GET, "/", host = "api.example", host = "other.example"
        })
        .err()
        .expect("duplicate predicate keys are rejected");
        assert!(duplicate.to_string().contains("duplicate `host`"), "{duplicate}");

        let duplicate = syn::parse2::<RouteAttr>(quote! {
            GET, "/", priority = 1, priority = 2
        })
        .err()
        .expect("duplicate priorities are rejected");
        assert!(duplicate.to_string().contains("duplicate `priority`"), "{duplicate}");

        let duplicate = syn::parse2::<RouteAttr>(quote! {
            GET, "/", headers(), headers()
        })
        .err()
        .expect("duplicate static response-header plans are rejected");
        assert!(duplicate.to_string().contains("duplicate `headers(...)`"), "{duplicate}");

        for invalid in [
            quote! { GET, "/", priority = "high" },
            quote! { GET, "/", priority = 1.5 },
            quote! { GET, "/", priority = 2147483648 },
            quote! { GET, "/", priority = -2147483649 },
            quote! { GET, "/", priority = 1_u8 },
        ] {
            let error = syn::parse2::<RouteAttr>(invalid).err().expect("malformed priorities are rejected");
            assert!(error.to_string().contains("priority"), "{error}");
        }

        let unknown = syn::parse2::<RouteAttr>(quote! { GET, "/", format = "application/json" })
            .err()
            .expect("unknown predicate keys are rejected");
        assert!(unknown.to_string().contains("unknown route argument"), "{unknown}");

        for invalid in [
            quote! { dynamic, "/items" },
            quote! { dynamic host = "api.example" },
            quote! { dynamic, host },
            quote! { dynamic, host = 42 },
        ] {
            let _ = syn::parse2::<RouteAttr>(invalid)
                .err()
                .expect("malformed dynamic predicate syntax is rejected");
        }
    }
}
