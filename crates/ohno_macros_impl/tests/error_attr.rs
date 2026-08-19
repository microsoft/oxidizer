// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::error_attr::*;
use syn::Item;

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    /// Expands the attribute and pretty-prints the result.
    ///
    /// The whole rewritten struct is snapshotted rather than searched for substrings, because what
    /// the attribute has to get right is the struct it hands to the derive: where the added field
    /// lands, what it is named, and what of the original survives beside it.
    fn rendered(item: Item) -> String {
        let expanded = expand(item);
        let file: syn::File = syn::parse2(expanded).expect("the expansion parses as a file");
        prettyplease::unparse(&file)
    }

    #[test]
    fn a_named_struct_gains_a_named_core() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T { path: String }
        }));
    }

    #[test]
    fn a_colliding_name_is_numbered() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T { ohno_core: u32, ohno_core_1: u32 }
        }));
    }

    #[test]
    fn a_tuple_struct_gains_a_trailing_core() {
        insta::assert_snapshot!(rendered(parse_quote!(
            struct T(String);
        )));
    }

    #[test]
    fn a_unit_struct_becomes_a_tuple_struct() {
        insta::assert_snapshot!(rendered(parse_quote!(
            struct T;
        )));
    }

    #[test]
    fn other_attributes_and_docs_survive() {
        insta::assert_snapshot!(rendered(parse_quote! {
            /// Documentation for T.
            #[derive(Clone)]
            #[display("failed for {path}")]
            struct T { path: String }
        }));
    }

    #[test]
    fn an_ordinary_doc_comment_is_left_alone() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T {
                /// Where the failure happened.
                path: String,
            }
        }));
    }

    #[test]
    fn a_marked_field_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T { path: String, #[error] mine: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_hand_written_reserved_marker_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T {
                path: String,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                mine: ohno::OhnoCore,
            }
        }));
    }

    #[test]
    fn no_constructors_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote! {
            #[no_constructors]
            struct T { path: String }
        }));
    }

    #[test]
    fn a_non_struct_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote!(
            enum T {
                A,
            }
        )));
    }

    #[test]
    fn a_rejected_struct_is_not_rewritten() {
        // A rejection stops the rewrite entirely, so nothing reaches the derive. Emitting a
        // half-rewritten struct beside the diagnostic would make rustc report the consequences of
        // the fault as well as the fault itself.
        let expanded = expand(parse_quote! {
            struct T { path: String, #[error] mine: ohno::OhnoCore }
        })
        .to_string();

        assert!(!expanded.contains("struct T"), "{expanded}");
    }
}
