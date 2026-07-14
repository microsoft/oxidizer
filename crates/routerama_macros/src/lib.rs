// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]

//! Procedural macros for the [`routerama`](https://docs.rs/routerama) crate.
//!
//! This crate is an implementation detail of the [`routerama`](https://docs.rs/routerama) crate. Please
//! see that crate for documentation.

use http_path_template::{Grammar, PathTemplate};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use routerama_build::trie::capture_field_names;
use routerama_build::{Generator, HttpMethod, Route, route_field_name};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Fields, GenericParam, Generics, Ident, ItemEnum, LitStr, Token, Variant};

/// Generates a [`routerama`](https://docs.rs/routerama) resolver for a route `enum`.
///
/// Apply `#[resolver(name = SomeResolver)]` to an `enum` and annotate each
/// variant with `#[route(METHOD, "path")]`; a capturing variant carries the
/// path's captures as `&'p str` fields. Generates a zero-sized `SomeResolver`
/// implementing `Resolver`, plus a `RouteMatch` impl on the enum.
#[cfg_attr(test, mutants::skip)]
#[proc_macro_attribute]
pub fn resolver(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = syn::parse_macro_input!(attr as ResolverAttr);
    let item = syn::parse_macro_input!(item as ItemEnum);
    expand(&attr.name, item).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// The `#[route(METHOD, "path")]` attribute on a variant.
struct RouteAttr {
    method: Ident,
    path: LitStr,
}

impl Parse for RouteAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let method: Ident = input.parse()?;
        let _comma: Token![,] = input.parse()?;
        let path: LitStr = input.parse()?;
        Ok(Self { method, path })
    }
}

/// The `name = Ident` argument of `#[resolver(name = Ident)]`.
struct ResolverAttr {
    name: Ident,
}

impl Parse for ResolverAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Err(input.error("expected `name = SomeResolver`"));
        }
        let key: Ident = input.parse()?;
        if key != "name" {
            return Err(syn::Error::new(key.span(), "expected `name = SomeResolver`"));
        }
        let _eq: Token![=] = input.parse()?;
        let name: Ident = input.parse()?;
        Ok(Self { name })
    }
}

/// Lowers the annotated `enum` into the resolver `impl`s, re-emitting the enum
/// (with the inert `#[route(...)]` attributes stripped) followed by the
/// generated `RouteMatch` impl and the zero-sized `Resolver`.
fn expand(resolver: &Ident, mut item: ItemEnum) -> syn::Result<TokenStream2> {
    let generic = has_capture_lifetime(&item.generics)?;

    let mut routes = Vec::new();
    let mut any_capture = false;
    for variant in &item.variants {
        any_capture |= matches!(&variant.fields, Fields::Named(fields) if !fields.named.is_empty());
        routes.extend(routes_for_variant(variant)?);
    }

    // The enum carries `<'p>` iff some variant captures a `&'p str` field. A
    // mismatch would otherwise surface only as an opaque "impl for a type that is
    // not this type" / "unused lifetime" error.
    if generic && !any_capture {
        return Err(syn::Error::new(
            item.ident.span(),
            "the enum declares a `'p` lifetime but no variant captures a path variable; remove `<'p>`",
        ));
    }
    if !generic && any_capture {
        return Err(syn::Error::new(
            item.ident.span(),
            "a variant captures a path variable, so the enum must declare the capture lifetime: `enum ... <'p>`",
        ));
    }

    let visibility = item.vis.to_token_stream();
    let mut generator = Generator::builder()
        .route_type(item.ident.to_string())
        .visibility(visibility.clone())
        .impls_only()
        .resolver_type(visibility, resolver.to_string())
        .build();
    generator.add_all(routes);
    let impls = generator.generate();

    // Strip the inert `#[route(...)]` attributes so the re-emitted enum compiles
    // (they carry no meaning once the routes are lowered).
    for variant in &mut item.variants {
        variant.attrs.retain(|attr| !attr.path().is_ident("route"));
    }

    Ok(quote! {
        #item
        #impls
    })
}

/// Validates the enum's generics and reports whether it carries the capture
/// lifetime `'p`. Only a single lifetime named `'p` is allowed.
fn has_capture_lifetime(generics: &Generics) -> syn::Result<bool> {
    let mut lifetime = false;
    for param in &generics.params {
        match param {
            GenericParam::Lifetime(def) => {
                if def.lifetime.ident != "p" {
                    return Err(syn::Error::new(def.lifetime.span(), "the capture lifetime must be named `'p`"));
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

/// Lowers a variant and each of its `#[route(...)]` attributes into [`Route`]s,
/// validating that the variant's fields match every route's path captures.
///
/// A variant may carry more than one `#[route]` to bind the same route name to
/// several method/path pairs (e.g. one handler for both `GET` and `HEAD`); each
/// must capture the same variables, since they share the one enum variant.
fn routes_for_variant(variant: &Variant) -> syn::Result<Vec<Route>> {
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
    for route_attr in route_attrs {
        let RouteAttr { method, path } = route_attr.parse_args()?;

        let path_str = path.value();
        let template = PathTemplate::parse(&path_str, Grammar::default().with_segment_affixes())
            .map_err(|error| syn::Error::new(path.span(), format!("invalid path template: {error}")))?;

        let mut expected: Vec<String> = capture_field_names(template.segments())
            .iter()
            .map(|name| route_field_name(name.join(".")))
            .collect();
        expected.sort();
        if expected != declared {
            return Err(syn::Error::new(
                path.span(),
                format!(
                    "variant `{}` fields {} do not match the path captures {} (each `{{capture}}` needs a matching `&'p str` field)",
                    variant.ident,
                    fmt_list(&declared),
                    fmt_list(&expected),
                ),
            ));
        }

        routes.push(Route::new(variant.ident.to_string(), method_from_ident(&method), template));
    }

    Ok(routes)
}

/// The named-field identifiers a variant declares (empty for a unit variant).
fn declared_fields(variant: &Variant) -> syn::Result<Vec<String>> {
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
            "tuple variants are not supported; use a struct variant with named `&'p str` fields",
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

/// Maps a method identifier (case-insensitive) to an [`HttpMethod`]; an
/// unrecognized token becomes [`HttpMethod::Custom`] carrying its upper-cased
/// name (e.g. `HEAD`).
fn method_from_ident(ident: &Ident) -> HttpMethod {
    match ident.to_string().to_ascii_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "PUT" => HttpMethod::Put,
        "POST" => HttpMethod::Post,
        "DELETE" => HttpMethod::Delete,
        "PATCH" => HttpMethod::Patch,
        other => HttpMethod::Custom(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_ok(name: &str, item: ItemEnum) -> String {
        let name: Ident = syn::parse_str(name).expect("valid ident");
        expand(&name, item).expect("valid").to_string()
    }

    #[test]
    fn expands_a_capturing_enum() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api<'p> {
                #[route(GET, "/books")]
                ListBooks,
                #[route(GET, "/books/{book}")]
                GetBook { book: &'p str },
            }
        };
        let code = expand_ok("ApiResolver", item);
        assert!(code.contains("fn resolve"), "{code}");
        assert!(code.contains("ApiResolver"), "{code}");
        // The enum is re-emitted (the attribute macro replaces the item)...
        assert!(code.contains("enum Api"), "the enum is re-emitted: {code}");
        // ...with the inert `#[route(...)]` attributes stripped.
        assert!(!code.contains("# [route"), "the `#[route]` attrs are stripped: {code}");
    }

    #[test]
    fn resolver_name_is_required() {
        // An empty `#[resolver]` (no `name = ...`) is rejected at the attribute.
        let empty = TokenStream2::new();
        assert!(
            syn::parse2::<ResolverAttr>(empty).is_err(),
            "a missing `name = ...` must be rejected"
        );
    }

    #[test]
    fn mismatched_fields_are_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api<'p> {
                #[route(GET, "/books/{book}")]
                GetBook { boook: &'p str },
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("mismatched fields must be rejected");
    }

    #[test]
    fn a_non_capturing_enum_expands() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api {
                #[route(GET, "/health")]
                Health,
            }
        };
        let code = expand_ok("ApiResolver", item);
        assert!(code.contains("ApiResolver"), "{code}");
    }

    #[test]
    fn capturing_variant_without_lifetime_is_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api {
                #[route(GET, "/books/{book}")]
                GetBook { book: &'p str },
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("a capture without `<'p>` must be rejected");
    }

    #[test]
    fn unused_lifetime_is_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api<'p> {
                #[route(GET, "/books")]
                ListBooks,
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("`<'p>` without any capture must be rejected");
    }

    #[test]
    fn empty_brace_variant_is_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api {
                #[route(GET, "/health")]
                Health {},
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("an empty-brace capture-less variant must be rejected");
    }

    #[test]
    fn a_resolver_attribute_with_the_wrong_key_is_rejected() {
        // `#[resolver(foo = X)]` — only `name` is accepted.
        let attr = quote::quote! { foo = ApiResolver };
        assert!(syn::parse2::<ResolverAttr>(attr).is_err(), "a non-`name` key must be rejected");
    }

    #[test]
    fn a_non_p_lifetime_is_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api<'a> {
                #[route(GET, "/books/{book}")]
                GetBook { book: &'a str },
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("a lifetime not named `'p` must be rejected");
    }

    #[test]
    fn a_type_parameter_is_rejected() {
        // Only a single `'p` lifetime is allowed — a type (or const) generic
        // parameter hits the `other` arm of `has_capture_lifetime`.
        let item: ItemEnum = syn::parse_quote! {
            enum Api<T> {
                #[route(GET, "/books")]
                ListBooks(T),
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("a type parameter must be rejected");
    }

    #[test]
    fn a_variant_without_a_route_attribute_is_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api {
                Health,
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("a variant missing `#[route]` must be rejected");
    }

    #[test]
    fn a_tuple_variant_is_rejected() {
        let item: ItemEnum = syn::parse_quote! {
            enum Api<'p> {
                #[route(GET, "/books/{book}")]
                GetBook(&'p str),
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let _ = expand(&name, item).expect_err("a tuple variant must be rejected");
    }

    #[test]
    fn a_unit_variant_with_a_capturing_route_is_rejected() {
        // The unit variant declares no fields but the path has a `{book}` capture,
        // so the field/capture mismatch error formats the empty declared set as
        // `{}` (exercises the empty branch of `fmt_list`).
        let item: ItemEnum = syn::parse_quote! {
            enum Api<'p> {
                #[route(GET, "/books/{book}")]
                GetBook,
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let error = expand(&name, item).expect_err("a unit variant with a capture must be rejected");
        assert!(error.to_string().contains("{}"), "empty field set formats as `{{}}`: {error}");
    }

    #[test]
    fn a_valid_resolver_attribute_parses() {
        // The success path of `ResolverAttr::parse` (`name = Ident`); the `expand`
        // unit tests pass a pre-built `Ident`, so only this exercises it.
        let attr = quote::quote! { name = ApiResolver };
        let parsed: ResolverAttr = syn::parse2(attr).expect("a `name = Ident` attribute parses");
        assert_eq!(parsed.name.to_string(), "ApiResolver");
    }

    #[test]
    fn a_valid_route_attribute_parses() {
        // The success path of `RouteAttr::parse` (`METHOD, "path"`).
        let attr = quote::quote! { GET, "/books/{book}" };
        let parsed: RouteAttr = syn::parse2(attr).expect("a `METHOD, \"path\"` attribute parses");
        assert_eq!(parsed.method.to_string(), "GET");
        assert_eq!(parsed.path.value(), "/books/{book}");
    }

    #[test]
    fn an_invalid_path_template_is_rejected() {
        // A malformed template surfaces the parse error from `PathTemplate::parse`.
        let item: ItemEnum = syn::parse_quote! {
            enum Api {
                #[route(GET, "/books/{unclosed")]
                GetBook,
            }
        };
        let name: Ident = syn::parse_quote!(ApiResolver);
        let error = expand(&name, item).expect_err("a malformed path template must be rejected");
        assert!(error.to_string().contains("invalid path template"), "{error}");
    }
}
