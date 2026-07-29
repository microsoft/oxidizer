// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of the `routerama` procedural macros.

use alloc::borrow::ToOwned as _;
use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{DeriveInput, Fields, GenericParam, Generics, Ident, ItemEnum, LitStr, Token, Variant};

use crate::trie::capture_field_names;
use crate::{Route, is_http_token, route_field_name};

mod field;
mod query;
mod resolver;

/// Expands `#[derive(FromQuery)]`.
#[must_use]
pub fn derive_from_query(input: TokenStream) -> TokenStream {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| query::expand_from_query(&input))
        .unwrap_or_else(syn::Error::into_compile_error)
}

/// Expands `#[derive(ToQuery)]`.
#[must_use]
pub fn derive_to_query(input: TokenStream) -> TokenStream {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| query::expand_to_query(&input))
        .unwrap_or_else(syn::Error::into_compile_error)
}

/// Expands `#[resolver]`.
#[must_use]
pub fn resolver(attr: TokenStream, item: TokenStream) -> TokenStream {
    syn::parse2::<ResolverAttr>(attr)
        .and_then(|attr| syn::parse2::<ItemEnum>(item).map(|item| (attr, item)))
        .and_then(|(attr, item)| resolver::expand_named(item, attr.name))
        .unwrap_or_else(syn::Error::into_compile_error)
}

#[derive(Default)]
struct ResolverAttr {
    name: Option<Ident>,
}

impl Parse for ResolverAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self::default());
        }

        let key: Ident = input.parse()?;
        if key != "name" {
            return Err(syn::Error::new(key.span(), "expected `name = ResolverType`"));
        }
        let _equals: Token![=] = input.parse()?;
        let name: Ident = input.parse()?;
        if name.to_string().starts_with("r#") {
            return Err(syn::Error::new(name.span(), "the resolver type name cannot be a raw identifier"));
        }
        let _trailing_comma = input.parse::<Option<Token![,]>>()?;
        if !input.is_empty() {
            return Err(input.error("unexpected resolver attribute argument"));
        }
        Ok(Self { name: Some(name) })
    }
}

/// The `#[route(METHOD, "path")]` attribute on a variant.
///
/// Identifier methods are normalized to uppercase; string methods are used
/// exactly as written and allow any RFC 9110 token.
struct RouteAttr {
    method: String,
    path: LitStr,
}

impl Parse for RouteAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
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
        Ok(Self { method, path })
    }
}

/// Validates the enum's generics and reports whether it carries the capture
/// lifetime `'p`. Only a single lifetime named `'p` is allowed.
pub(crate) fn has_capture_lifetime(generics: &Generics) -> syn::Result<bool> {
    if let Some(where_clause) = &generics.where_clause {
        return Err(syn::Error::new(
            where_clause.span(),
            "#[resolver] does not support a `where` clause because its generated route type must be valid for every request lifetime",
        ));
    }
    let mut lifetime = false;
    for param in &generics.params {
        let attrs = match param {
            GenericParam::Lifetime(param) => &param.attrs,
            GenericParam::Type(param) => &param.attrs,
            GenericParam::Const(param) => &param.attrs,
        };
        if let Some(attribute) = attrs
            .iter()
            .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
        {
            return Err(syn::Error::new(
                attribute.span(),
                "`#[resolver]` does not support conditionally compiled generic parameters",
            ));
        }
        match param {
            GenericParam::Lifetime(def) => {
                if def.lifetime.ident != "p" {
                    return Err(syn::Error::new(def.lifetime.span(), "the capture lifetime must be named `'p`"));
                }
                if !def.bounds.is_empty() {
                    return Err(syn::Error::new(
                        def.bounds.span(),
                        "the capture lifetime `'p` cannot have bounds because generated routers accept any request lifetime",
                    ));
                }
                lifetime = true;
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "#[resolver] supports only a single `'p` lifetime parameter",
                ));
            }
        }
    }
    Ok(lifetime)
}

/// Whether `variant` carries at least one `#[route(...)]` attribute, i.e. it is
/// a *static* route variant (as opposed to a dynamic one, registered at run
/// time).
pub(crate) fn has_route_attr(variant: &Variant) -> bool {
    variant.attrs.iter().any(|attr| attr.path().is_ident("route"))
}

/// Lowers a variant and each of its `#[route(...)]` attributes into [`Route`]s,
/// validating that the variant's fields match every route's path captures.
///
/// A variant may carry more than one `#[route]` to bind the same route name to
/// several method/path pairs (e.g. one handler for both `GET` and `HEAD`); each
/// must capture the same variables, since they share the one enum variant.
pub(crate) fn routes_for_variant(variant: &Variant) -> syn::Result<Vec<Route>> {
    let route_attrs: Vec<_> = variant.attrs.iter().filter(|attr| attr.path().is_ident("route")).collect();
    if route_attrs.is_empty() {
        return Err(syn::Error::new(
            variant.span(),
            format!("variant `{}` is missing a `#[route(METHOD, \"path\")]` attribute", variant.ident),
        ));
    }

    let mut declared = declared_fields(variant)?;
    declared.sort();

    let mut routes = Vec::with_capacity(route_attrs.len());
    let mut first_capture_keys: Option<Vec<String>> = None;
    for route_attr in route_attrs {
        let RouteAttr { method, path } = route_attr.parse_args()?;

        let path_str = path.value();
        let template = PathTemplate::parse(&path_str, Grammar::default().with_segment_affixes())
            .map_err(|error| syn::Error::new(path.span(), format!("invalid path template: {error}")))?;

        let captures = capture_field_names(template.segments());
        let mut capture_keys: Vec<String> = captures.iter().map(|name| name.join(".")).collect();
        capture_keys.sort();
        if let Some(first) = &first_capture_keys {
            if *first != capture_keys {
                return Err(syn::Error::new(
                    path.span(),
                    format!(
                        "variant `{}` has routes that capture different path variables ({} vs {}); every route on one variant must capture the same variables",
                        variant.ident,
                        fmt_list(first),
                        fmt_list(&capture_keys),
                    ),
                ));
            }
        } else {
            first_capture_keys = Some(capture_keys);
        }

        let mut expected: Vec<String> = captures.iter().map(|name| route_field_name(name.join("."))).collect();
        expected.sort();
        if expected != declared {
            return Err(syn::Error::new(
                path.span(),
                format!(
                    "variant `{}` fields {} do not match the path captures {} (each `{{capture}}` needs a matching named field)",
                    variant.ident,
                    fmt_list(&declared),
                    fmt_list(&expected),
                ),
            ));
        }

        routes.push(Route::new(variant.ident.to_string(), method, template));
    }

    Ok(routes)
}

/// The named-field identifiers a variant declares (empty for a unit variant).
pub(crate) fn declared_fields(variant: &Variant) -> syn::Result<Vec<String>> {
    match &variant.fields {
        Fields::Unit => Ok(Vec::new()),
        Fields::Named(named) if named.named.is_empty() => Err(syn::Error::new(
            variant.fields.span(),
            format!(
                "write the capture-less route `{}` as a unit variant (`{0}`), not empty braces (`{0} {{}}`)",
                variant.ident
            ),
        )),
        Fields::Named(named) => Ok(named
            .named
            .iter()
            .map(|field| field.ident.as_ref().expect("named field has an identifier").to_string())
            .collect()),
        Fields::Unnamed(_) => Err(syn::Error::new(
            variant.fields.span(),
            "tuple variants are not supported; use a struct variant with named fields",
        )),
    }
}

fn fmt_list(items: &[String]) -> String {
    if items.is_empty() {
        "{}".to_owned()
    } else {
        format!("{{{}}}", items.join(", "))
    }
}

/// Maps a method identifier to its upper-cased HTTP method token (e.g. `get` →
/// `"GET"`, `HEAD` → `"HEAD"`), which a route matches on case-sensitively.
fn method_token(ident: &Ident) -> String {
    let spelling = ident.to_string();
    spelling.strip_prefix("r#").unwrap_or(&spelling).to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use quote::quote;

    use super::*;

    /// Parses a single enum variant from source for exercising the shared
    /// lowering helpers (`routes_for_variant`, `declared_fields`) directly.
    fn variant(src: proc_macro2::TokenStream) -> Variant {
        syn::parse2(src).expect("a valid variant")
    }

    #[test]
    fn a_valid_route_attribute_parses() {
        // The success path of `RouteAttr::parse` (`METHOD, "path"`).
        let attr = quote! { GET, "/books/{book}" };
        let parsed: RouteAttr = syn::parse2(attr).expect("a `METHOD, \"path\"` attribute parses");
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path.value(), "/books/{book}");
    }

    #[test]
    fn a_hyphenated_method_string_parses_exactly() {
        let parsed: RouteAttr = syn::parse2(quote! { "M-SEARCH", "/devices" }).expect("a valid HTTP token parses");
        assert_eq!(parsed.method, "M-SEARCH");

        let error = syn::parse2::<RouteAttr>(quote! { "BAD METHOD", "/devices" })
            .err()
            .expect("spaces are not allowed in an HTTP token");
        assert!(error.to_string().contains("RFC 9110"), "{error}");
    }

    #[test]
    fn identifier_methods_are_normalized_and_validated() {
        let parsed: RouteAttr = syn::parse2(quote! { r#match, "/items" }).expect("raw method identifier parses");
        assert_eq!(parsed.method, "MATCH");

        let non_token = Ident::new("\u{03bb}", proc_macro2::Span::call_site());
        let error = syn::parse2::<RouteAttr>(quote! { #non_token, "/items" })
            .err()
            .expect("non-token identifier methods are rejected");
        assert!(error.to_string().contains("RFC 9110"), "{error}");
    }

    #[test]
    fn resolver_accepts_no_arguments() {
        let attr = syn::parse2::<ResolverAttr>(quote! {}).expect("`#[resolver]` (bare) is accepted");
        assert!(attr.name.is_none());
    }

    #[test]
    fn resolver_accepts_an_explicit_name_and_rejects_other_arguments() {
        let attr = syn::parse2::<ResolverAttr>(quote! { name = ApiResolver }).expect("explicit resolver name is accepted");
        assert_eq!(attr.name.expect("name is present"), "ApiResolver");
        let attr = syn::parse2::<ResolverAttr>(quote! { name = ApiResolver, }).expect("a trailing comma is accepted");
        assert_eq!(attr.name.expect("name is present"), "ApiResolver");

        for invalid in [
            quote! { ApiResolver },
            quote! { type_name = ApiResolver },
            quote! { name = r#type },
            quote! { name = ApiResolver, extra },
        ] {
            assert!(syn::parse2::<ResolverAttr>(invalid).is_err());
        }
    }

    #[test]
    fn routes_for_variant_lowers_every_method() {
        // One variant carrying every recognized verb plus a custom one, so
        // `method_from_ident` maps each arm (`GET`/`PUT`/`POST`/`DELETE`/`PATCH`
        // and the `Custom` fallback for `HEAD`).
        let variant = variant(quote! {
            #[route(GET, "/x")]
            #[route(PUT, "/x")]
            #[route(POST, "/x")]
            #[route(DELETE, "/x")]
            #[route(PATCH, "/x")]
            #[route(HEAD, "/x")]
            X
        });
        let routes = routes_for_variant(&variant).expect("valid routes");
        let methods: Vec<&str> = routes.iter().map(Route::method).collect();
        assert_eq!(methods, vec!["GET", "PUT", "POST", "DELETE", "PATCH", "HEAD"]);
    }

    #[test]
    fn mismatched_fields_are_rejected() {
        // A declared field name that no capture matches: the mismatch message
        // formats the (non-empty) declared set via `fmt_list`.
        let variant = variant(quote! {
            #[route(GET, "/books/{book}")]
            GetBook { boook: &'p str }
        });
        let error = routes_for_variant(&variant).expect_err("mismatched fields must be rejected");
        assert!(error.to_string().contains("{boook}"), "declared field set is listed: {error}");
    }

    #[test]
    fn alternate_routes_must_use_the_same_original_capture_names() {
        let variant = variant(quote! {
            #[route(GET, "/v1/shelves/{shelf.id}")]
            #[route(GET, "/v2/shelves/{shelf_id}")]
            GetShelf { shelf_id: &'p str }
        });
        let error = routes_for_variant(&variant).expect_err("distinct source capture names must be rejected");
        let message = error.to_string();
        assert!(message.contains("different path variables"), "{message}");
        assert!(message.contains("shelf.id"), "{message}");
        assert!(message.contains("shelf_id"), "{message}");
    }

    #[test]
    fn a_unit_variant_with_a_capturing_route_is_rejected() {
        // The unit variant declares no fields but the path has a `{book}`
        // capture, so the mismatch message formats the empty declared set as `{}`
        // (the empty branch of `fmt_list`).
        let variant = variant(quote! {
            #[route(GET, "/books/{book}")]
            GetBook
        });
        let error = routes_for_variant(&variant).expect_err("a unit variant with a capture must be rejected");
        assert!(error.to_string().contains("{}"), "empty field set formats as `{{}}`: {error}");
    }

    #[test]
    fn an_empty_brace_variant_is_rejected() {
        // `Health {}` (empty braces) must be written as a unit variant `Health`.
        let variant = variant(quote! {
            #[route(GET, "/health")]
            Health {}
        });
        let _ = routes_for_variant(&variant).expect_err("an empty-brace capture-less variant must be rejected");
    }

    #[test]
    fn a_tuple_variant_is_rejected() {
        let variant = variant(quote! {
            #[route(GET, "/books/{book}")]
            GetBook(&'p str)
        });
        let _ = routes_for_variant(&variant).expect_err("a tuple variant must be rejected");
    }

    #[test]
    fn a_variant_without_a_route_attribute_is_rejected() {
        let variant = variant(quote! { Health });
        let _ = routes_for_variant(&variant).expect_err("a variant missing `#[route]` must be rejected");
    }

    #[test]
    fn an_invalid_path_template_is_rejected() {
        let variant = variant(quote! {
            #[route(GET, "/books/{unclosed")]
            GetBook
        });
        let error = routes_for_variant(&variant).expect_err("a malformed path template must be rejected");
        assert!(error.to_string().contains("invalid path template"), "{error}");
    }

    #[test]
    fn has_capture_lifetime_accepts_p_and_none() {
        let with_p: Generics = syn::parse_quote! { <'p> };
        assert!(has_capture_lifetime(&with_p).expect("valid"), "`<'p>` carries the capture lifetime");
        let empty: Generics = syn::parse_quote! {};
        assert!(
            !has_capture_lifetime(&empty).expect("valid"),
            "no generics means no capture lifetime"
        );
    }

    #[test]
    fn a_non_p_lifetime_is_rejected() {
        let generics: Generics = syn::parse_quote! { <'a> };
        let _ = has_capture_lifetime(&generics).expect_err("a lifetime not named `'p` must be rejected");
    }

    #[test]
    fn a_type_parameter_is_rejected() {
        // Only a single `'p` lifetime is allowed — a type generic hits the
        // `other` arm of `has_capture_lifetime`.
        let generics: Generics = syn::parse_quote! { <T> };
        let _ = has_capture_lifetime(&generics).expect_err("a type parameter must be rejected");
    }

    #[test]
    fn lifetime_bounds_and_where_clauses_are_rejected() {
        let bounded: Generics = syn::parse_quote! { <'p: 'static> };
        let error = has_capture_lifetime(&bounded).expect_err("a bounded request lifetime is unsupported");
        assert!(error.to_string().contains("cannot have bounds"), "{error}");

        let item: ItemEnum = syn::parse_quote! {
            enum Route<'p>
            where
                'p: 'static,
            {}
        };
        let error = has_capture_lifetime(&item.generics).expect_err("a where clause is unsupported");
        assert!(error.to_string().contains("where"), "{error}");
    }

    #[test]
    fn conditionally_compiled_generic_parameters_are_rejected() {
        for generics in [
            syn::parse_quote! { <#[cfg(feature = "x")] 'p> },
            syn::parse_quote! { <#[cfg_attr(feature = "x", allow(dead_code))] 'p> },
            syn::parse_quote! { <#[cfg(feature = "x")] T> },
            syn::parse_quote! { <#[cfg(feature = "x")] const N: usize> },
        ] {
            let error = has_capture_lifetime(&generics).expect_err("conditional generics alter generated type arity");
            assert!(error.to_string().contains("generic parameters"), "{error}");
        }
    }
}
