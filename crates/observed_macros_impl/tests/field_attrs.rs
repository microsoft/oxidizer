// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(missing_docs, reason = "Test code")]

use observed_macros_impl::internals::{mentions_any_type_param, option_inner_type, strip_reference};
use syn::Ident;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod coverage_tests {
    use super::*;

    fn ty(s: &str) -> syn::Type {
        syn::parse_str(s).expect("parse type")
    }

    #[test]
    fn option_inner_type_rejects_non_option_shapes() {
        // qualified self (`<T as Trait>::Assoc`).
        assert!(option_inner_type(&ty("<i32 as Copy>::Output")).is_none());
        // `Option` without angle-bracketed arguments.
        assert!(option_inner_type(&ty("Option")).is_none());
        // more than one generic argument.
        assert!(option_inner_type(&ty("Option<u8, u16>")).is_none());
        // a non-type (lifetime) generic argument.
        assert!(option_inner_type(&ty("Option<'a>")).is_none());
    }

    #[test]
    fn strip_reference_peels_every_reference_layer() {
        fn rendered(ty: &syn::Type) -> String {
            quote::ToTokens::to_token_stream(ty).to_string()
        }

        assert_eq!(rendered(strip_reference(&ty("&T"))), "T");
        assert_eq!(rendered(strip_reference(&ty("& &mut T"))), "T");
        // A non-reference type is returned unchanged.
        assert_eq!(rendered(strip_reference(&ty("Vec<T>"))), rendered(&ty("Vec<T>")));
    }

    #[test]
    fn mentions_any_type_param_descends_into_token_groups() {
        let param: Ident = syn::parse_str("T").expect("parse ident");

        // Array and tuple types nest their contents in a token `Group`, which is
        // the only way the walker recurses.
        assert!(mentions_any_type_param(&ty("[T; 4]"), std::slice::from_ref(&param)));
        assert!(mentions_any_type_param(&ty("(u8, T)"), std::slice::from_ref(&param)));
        assert!(!mentions_any_type_param(&ty("[u8; 4]"), std::slice::from_ref(&param)));
    }
}
