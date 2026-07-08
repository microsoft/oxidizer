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
use quote::quote;
use routerama_build::{Generator, HttpMethod, RouteRule};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token, Visibility, braced};

/// A parsed `routes!` invocation: the target enum's visibility and name, its
/// route table (already lowered to [`routerama_build::RouteRule`]s), and an
/// optional companion router struct.
struct RoutesDef {
    visibility: Visibility,
    name: Ident,
    routes: Vec<RouteRule>,
    /// The optional `[visibility] struct Name;` companion: when present, a
    /// zero-sized `Router` (and a `RouteMatch` impl on the enum) is generated
    /// under that name and visibility.
    companion: Option<(Visibility, Ident)>,
}

impl Parse for RoutesDef {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        // `[visibility] enum Name { <routes> }`
        let visibility: Visibility = input.parse()?;
        let _enum: Token![enum] = input.parse()?;
        let name: Ident = input.parse()?;

        let body;
        let _brace = braced!(body in input);

        let mut routes = Vec::new();
        while !body.is_empty() {
            let route_name: Ident = body.parse()?;
            let method: Ident = body.parse()?;
            let template: LitStr = body.parse()?;
            // Entries are separated by `,`; the
            // separator after the last entry is optional.
            if body.peek(Token![,]) {
                let _comma: Token![,] = body.parse()?;
            }

            let parsed = PathTemplate::parse(template.value(), Grammar::default().with_segment_affixes())
                .map_err(|error| syn::Error::new(template.span(), format!("invalid path template: {error}")))?;
            routes.push(RouteRule::new(route_name.to_string(), method_from_ident(&method), parsed));
        }

        // Optional companion router: `[visibility] struct Name;`. Present ⇒ emit a
        // ZST `Router` (+ `RouteMatch` on the enum) so the static router composes
        // through the runtime routing traits.
        let companion = if input.is_empty() {
            None
        } else {
            let router_visibility: Visibility = input.parse()?;
            let _struct: Token![struct] = input.parse()?;
            let router_name: Ident = input.parse()?;
            let _semi: Token![;] = input.parse()?;
            Some((router_visibility, router_name))
        };

        Ok(Self {
            visibility,
            name,
            routes,
            companion,
        })
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

#[cfg_attr(test, mutants::skip)]
#[proc_macro]
pub fn routes(input: TokenStream) -> TokenStream {
    let RoutesDef {
        visibility,
        name,
        routes,
        companion,
    } = syn::parse_macro_input!(input as RoutesDef);
    let mut builder = Generator::builder().route_type(name.to_string()).visibility(quote! { #visibility });
    if let Some((router_visibility, router_name)) = companion {
        builder = builder.router_type(quote! { #router_visibility }, router_name.to_string());
    }
    let mut generator = builder.build();
    generator.add_all(routes);
    generator.generate().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_every_route_entry() {
        // Two entries: the parse loop must iterate once per entry — deleting the
        // `!` in `while !body.is_empty()` would parse zero routes.
        let def: RoutesDef = syn::parse2(quote! {
            pub enum BookRoute {
                ListBooks GET "/books",
                GetBook   GET "/books/{book}",
            }
        })
        .expect("valid routes! input");
        assert_eq!(def.routes.len(), 2);
        assert_eq!(def.name.to_string(), "BookRoute");
        assert!(def.companion.is_none());
    }

    #[test]
    fn parse_reads_the_optional_companion_struct() {
        let def: RoutesDef = syn::parse2(quote! {
            enum Api {
                Health GET "/health"
            }
            struct ApiRouter;
        })
        .expect("valid routes! input with a companion struct");
        assert_eq!(def.routes.len(), 1);
        let (_, name) = def.companion.expect("companion struct parsed");
        assert_eq!(name.to_string(), "ApiRouter");
    }
}
