// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::format;

use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::quote;

#[derive(Clone, Copy)]
pub(crate) enum RuntimeCapability {
    #[cfg(feature = "query")]
    Query,
    #[cfg(feature = "resolve")]
    Resolve,
    #[cfg(feature = "route")]
    Route,
}

pub(crate) fn runtime_path(capability: RuntimeCapability) -> TokenStream2 {
    runtime_path_for(crate_name("routerama").ok(), capability)
}

#[cfg(feature = "route")]
pub(crate) fn response_path() -> TokenStream2 {
    response_path_for(crate_name("routerama").ok())
}

fn runtime_root(found: Option<FoundCrate>) -> TokenStream2 {
    match found {
        Some(FoundCrate::Name(name)) => {
            let name = name.replace('-', "_");
            let ident = syn::parse_str::<Ident>(&name)
                .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
                .expect("Cargo dependency aliases are valid Rust identifiers, possibly requiring raw syntax");
            quote! { ::#ident }
        }
        Some(FoundCrate::Itself) | None => quote! { ::routerama },
    }
}

pub(crate) fn runtime_path_for(found: Option<FoundCrate>, capability: RuntimeCapability) -> TokenStream2 {
    let root = runtime_root(found);
    match capability {
        #[cfg(feature = "query")]
        RuntimeCapability::Query => quote! { #root::query::__private },
        #[cfg(feature = "resolve")]
        RuntimeCapability::Resolve => quote! { #root::resolve::__private },
        #[cfg(feature = "route")]
        RuntimeCapability::Route => quote! { #root::route::__private },
    }
}

#[cfg(feature = "route")]
fn response_path_for(found: Option<FoundCrate>) -> TokenStream2 {
    let root = runtime_root(found);
    quote! { #root::response }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned as _;
    use alloc::string::ToString as _;

    use super::*;

    #[test]
    fn renamed_dependency_paths_include_the_owning_module() {
        let renamed = || Some(FoundCrate::Name("rr".to_owned()));

        #[cfg(feature = "query")]
        assert_eq!(
            runtime_path_for(renamed(), RuntimeCapability::Query).to_string(),
            ":: rr :: query :: __private"
        );
        #[cfg(feature = "resolve")]
        assert_eq!(
            runtime_path_for(renamed(), RuntimeCapability::Resolve).to_string(),
            ":: rr :: resolve :: __private"
        );
        #[cfg(feature = "route")]
        assert_eq!(
            runtime_path_for(renamed(), RuntimeCapability::Route).to_string(),
            ":: rr :: route :: __private"
        );
        #[cfg(feature = "route")]
        assert_eq!(response_path_for(renamed()).to_string(), ":: rr :: response");
    }
}
