// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of the `routerama` procedural macros.

use proc_macro2::TokenStream;
#[cfg(feature = "query")]
use syn::DeriveInput;
#[cfg(feature = "resolve")]
use syn::ItemEnum;

#[cfg(any(feature = "resolve", feature = "route"))]
mod field;
#[cfg(any(feature = "resolve", feature = "route"))]
mod predicate_value;
#[cfg(feature = "query")]
mod query;
#[cfg(any(feature = "resolve", feature = "route"))]
mod resolver;
#[cfg(feature = "resolve")]
mod resolver_attr;
#[cfg(any(feature = "resolve", feature = "route"))]
mod route_attr;
#[cfg(feature = "route")]
mod router;
#[cfg(feature = "route")]
mod router_attr;
mod runtime;
#[cfg(any(feature = "resolve", feature = "route"))]
mod variant;

#[cfg(feature = "resolve")]
use resolver_attr::ResolverAttr;
#[cfg(any(feature = "resolve", feature = "route"))]
use route_attr::{RouteAttr, RouteDeclaration, RouteTarget, route_declaration};
#[cfg(feature = "route")]
use route_attr::{RoutePredicates, RoutePriority, StaticHeader, StaticHeaderOperation, differing_static_header, same_static_headers};
#[cfg(feature = "route")]
use router_attr::RouterAttr;
#[cfg(any(feature = "resolve", feature = "route"))]
use variant::{declared_fields, has_capture_lifetime, routes_for_variant};

/// Expands `#[derive(FromQuery)]`.
#[must_use]
#[cfg(feature = "query")]
pub fn derive_from_query(input: TokenStream) -> TokenStream {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| query::expand_from_query(&input))
        .unwrap_or_else(syn::Error::into_compile_error)
}

/// Expands `#[derive(ToQuery)]`.
#[must_use]
#[cfg(feature = "query")]
pub fn derive_to_query(input: TokenStream) -> TokenStream {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| query::expand_to_query(&input))
        .unwrap_or_else(syn::Error::into_compile_error)
}

/// Expands `#[resolver]`.
#[must_use]
#[cfg(feature = "resolve")]
pub fn resolver(attr: TokenStream, item: TokenStream) -> TokenStream {
    syn::parse2::<ResolverAttr>(attr)
        .and_then(|attr| syn::parse2::<ItemEnum>(item).map(|item| (attr, item)))
        .and_then(|(attr, item)| resolver::expand_named(item, attr.name, runtime::RuntimeCapability::Resolve))
        .unwrap_or_else(syn::Error::into_compile_error)
}

/// Expands `#[router]`.
#[must_use]
#[cfg(feature = "route")]
pub fn router(attr: TokenStream, item: TokenStream) -> TokenStream {
    syn::parse2::<RouterAttr>(attr)
        .and_then(|attr| syn::parse2::<syn::ItemImpl>(item).map(|item| (attr, item)))
        .and_then(|(attr, item)| router::expand_with_data(item, attr.state, attr.erased_mounts, attr.tower, attr.heterogeneous_data))
        .unwrap_or_else(syn::Error::into_compile_error)
}
