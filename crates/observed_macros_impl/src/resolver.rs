// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Resolution of the `observed` runtime crate path that both generators emit into.
//!
//! The generated code has to name the runtime crate, and a consumer is free to
//! rename its dependency:
//!
//! ```toml
//! telemetry = { package = "observed", version = "0.24" }
//! ```
//!
//! after which `::observed` names nothing in that crate. [`runtime_path`] asks
//! Cargo what the dependency is actually called in the crate currently being
//! compiled, so the expansion reaches the runtime under whichever name the
//! consumer chose.

use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Ident, TokenStream};
use quote::quote;

/// The path the generated code uses to reach the `observed` runtime crate.
pub(crate) fn runtime_path() -> TokenStream {
    runtime_path_for(crate_name("observed").ok())
}

/// Turns a resolution result into the leading path segment of the runtime crate.
///
/// [`FoundCrate::Itself`] is the `observed` crate expanding its own events; it
/// keeps `::observed`, which resolves through the `extern crate self as observed`
/// declaration in that crate's root. `None` is the no-manifest case (a caller
/// Cargo cannot tell us about, such as a direct `syn` test harness), where
/// `::observed` is the only sensible guess and matches the historical behavior.
fn runtime_path_for(found: Option<FoundCrate>) -> TokenStream {
    match found {
        Some(FoundCrate::Name(name)) => {
            // Cargo permits `-` in a dependency alias, which is not a Rust identifier;
            // an alias that collides with a keyword needs the raw form.
            let name = name.replace('-', "_");
            let ident = syn::parse_str::<Ident>(&name)
                .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
                .expect("Cargo dependency aliases are valid Rust identifiers, possibly requiring raw syntax");
            quote! { ::#ident }
        }
        Some(FoundCrate::Itself) | None => quote! { ::observed },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renamed_dependency_is_reached_under_the_name_the_consumer_chose() {
        assert_eq!(
            runtime_path_for(Some(FoundCrate::Name("telemetry".to_owned()))).to_string(),
            ":: telemetry"
        );
    }

    #[test]
    fn a_dependency_alias_spelled_with_dashes_becomes_the_module_name_cargo_gives_it() {
        assert_eq!(
            runtime_path_for(Some(FoundCrate::Name("my-observed".to_owned()))).to_string(),
            ":: my_observed"
        );
    }

    #[test]
    fn a_dependency_alias_that_is_a_keyword_is_escaped_as_a_raw_identifier() {
        assert_eq!(runtime_path_for(Some(FoundCrate::Name("type".to_owned()))).to_string(), ":: r#type");
    }

    #[test]
    fn the_runtime_crate_expanding_its_own_events_keeps_the_canonical_path() {
        // `observed` reaches itself through `extern crate self as observed`.
        assert_eq!(runtime_path_for(Some(FoundCrate::Itself)).to_string(), ":: observed");
    }

    #[test]
    fn a_caller_cargo_cannot_resolve_falls_back_to_the_canonical_path() {
        assert_eq!(runtime_path_for(None).to_string(), ":: observed");
    }

    #[test]
    fn the_production_resolver_always_yields_a_path() {
        assert!(!runtime_path().is_empty());
    }
}
