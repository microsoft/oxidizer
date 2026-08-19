// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::derive_error::display::*;
use ohno_macros_impl::diagnostics::Errors;
use quote::quote;

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
// `unsupported operation: `open` not available when isolation is enabled`
#[cfg(all(test, not(miri)))]
mod tests {
    use ohno_macros_impl::derive_error::ast::Ast;
    use ohno_macros_impl::derive_error::parse;
    use syn::parse_quote;

    use super::*;

    fn ast_of(input: syn::DeriveInput) -> Ast {
        let mut errors = Errors::default();
        let ast = parse::parse(input, &mut errors).expect("the input parses");
        assert!(errors.is_empty());
        ast
    }

    /// Lowers the `#[display(...)]` of `input` and renders the outcome.
    ///
    /// The message and the diagnostics go into one snapshot because they are one outcome: a
    /// template either lowers to a message or reports why it cannot. Showing one half alone cannot
    /// tell "lowered cleanly" apart from "lowered and also complained", and a substring assertion
    /// on either half passes on an expansion that is wrong everywhere it did not look.
    ///
    /// A snapshot with no `fn message` is a template that did not lower at all.
    fn lowered(input: syn::DeriveInput) -> String {
        let ast = ast_of(input);
        let fields = Referenceable::new(&ast.fields);
        let mut errors = Errors::default();
        let message = lower(ast.display.as_ref().expect("a display attribute"), &fields, &mut errors);

        let message = message.map(|message| {
            let expr = message.render();
            quote! {
                fn message() {
                    let _ = #expr;
                }
            }
        });
        let diagnostics = errors.into_compile_error();

        let file: syn::File = syn::parse2(quote!(#message #diagnostics)).expect("the outcome parses as a file");
        prettyplease::unparse(&file)
    }

    #[test]
    fn a_static_template_lowers_to_a_literal() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("Static error message")]
            struct T { inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn escapes_are_resolved_for_a_literal_message() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("a {{literal}} brace")]
            struct T { inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_named_placeholder_becomes_a_field_access() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("failed for {path}")]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_format_spec_survives_lowering() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("rules: {rules:?}")]
            struct T { rules: Vec<String>, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_positional_argument_is_scoped_and_parenthesized() {
        // The parentheses are load-bearing: a bare `&self.count * 2` would multiply the reference
        // rather than the field.
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{}", count * 2)]
            struct T { count: u32, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_tuple_field_is_named_by_index() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{0} and {}", 1.abs())]
            struct T(String, i32, ohno::OhnoCore);
        }));
    }

    #[test]
    fn an_argument_may_call_a_method_of_self() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{}", describe())]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn nothing_is_referenceable_when_every_field_is_generated() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {path}")]
            struct T(#[doc = " ohno::generated-core@7f3d9c2a"] ohno::OhnoCore);
        }));
    }

    #[test]
    fn the_added_core_is_not_referenceable() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {1}")]
            struct T(String, #[doc = " ohno::generated-core@7f3d9c2a"] ohno::OhnoCore);
        }));
    }

    #[test]
    fn a_declared_core_stays_referenceable() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{path}, carrying {carried}")]
            struct T {
                path: String,
                carried: ohno::OhnoCore,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        }));
    }

    #[test]
    fn an_unknown_placeholder_lists_the_available_fields() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {pth}")]
            struct T {
                path: String,
                code: u32,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        }));
    }

    #[test]
    fn a_declared_core_is_offered_as_an_available_field() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {pth}")]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn an_unknown_argument_root_is_reported() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {}", pth.display())]
            struct T { path: String, code: u32, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_raw_identifier_is_offered_with_its_prefix() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad: {typ}")]
            struct T {
                r#type: String,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        }));
    }

    #[test]
    fn a_self_prefixed_argument_is_reported() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {}", self.path.display())]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn an_unsupported_argument_root_is_reported() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad: {}", Self::LABEL.len())]
            struct T { inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn an_unbalanced_brace_stops_the_lowering() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: {path")]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_stray_closing_brace_stops_the_lowering() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("bad path: path}")]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn too_few_arguments_are_reported() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{} {}", path)]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn an_unconsumed_argument_is_reported() {
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{}", path, code)]
            struct T { path: String, code: u32, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn every_fault_in_one_template_is_reported_together() {
        // Three faults, reported together: two unknown placeholders and one argument that no
        // placeholder consumes. Reporting only the first would cost three compile cycles to fix.
        insta::assert_snapshot!(lowered(parse_quote! {
            #[display("{one} {two}", extra)]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }
}
