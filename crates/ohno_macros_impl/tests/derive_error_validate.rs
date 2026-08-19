// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::derive_error::model::Model;
use ohno_macros_impl::derive_error::parse::member_name;
use ohno_macros_impl::derive_error::validate::*;
use ohno_macros_impl::diagnostics::Errors;

#[cfg(test)]
mod tests {
    use ohno_macros_impl::derive_error::parse;
    use syn::parse_quote;

    use super::*;

    /// Validates `input`, returning the model and the faults.
    fn validate_of(input: syn::DeriveInput) -> (Option<Model>, String) {
        let mut errors = Errors::default();
        let ast = parse::parse(input, &mut errors).expect("the input parses");
        let model = validate(ast, &mut errors);
        (model, errors.into_compile_error().to_string())
    }

    fn model_of(input: syn::DeriveInput) -> Model {
        let (model, faults) = validate_of(input);
        assert!(faults.is_empty(), "unexpected faults: {faults}");
        model.expect("a model")
    }

    fn faults_of(input: syn::DeriveInput) -> String {
        let (_, faults) = validate_of(input);
        assert!(!faults.is_empty(), "expected a fault");
        faults
    }

    #[test]
    fn a_lone_ohno_core_field_is_the_core() {
        let model = model_of(parse_quote! {
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert_eq!(member_name(&model.shape.core().member), "inner");
        assert_eq!(model.shape.data().count(), 1);
    }

    #[test]
    fn a_marked_field_wins_over_its_type() {
        let model = model_of(parse_quote! {
            struct T { metadata: ohno::OhnoCore, #[error] main: Core }
        });

        assert_eq!(member_name(&model.shape.core().member), "main");
    }

    #[test]
    fn a_marked_field_is_not_type_checked() {
        let model = model_of(parse_quote! {
            struct T { path: String, #[error] inner: Core }
        });

        assert_eq!(member_name(&model.shape.core().member), "inner");
    }

    #[test]
    fn the_generated_marker_wins_over_a_declared_core() {
        let model = model_of(parse_quote! {
            struct T {
                carried: ohno::OhnoCore,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert_eq!(member_name(&model.shape.core().member), "ohno_core");
        assert_eq!(model.shape.data().count(), 1);
    }

    #[test]
    fn the_flags_invert_into_the_model() {
        let plain = model_of(parse_quote!(
            struct T {
                inner: ohno::OhnoCore,
            }
        ));
        assert!(plain.debug);
        assert!(plain.constructors);

        let suppressed = model_of(parse_quote! {
            #[no_debug]
            #[no_constructors]
            struct T { inner: ohno::OhnoCore }
        });
        assert!(!suppressed.debug);
        assert!(!suppressed.constructors);
    }

    #[test]
    fn conversions_reach_the_model() {
        let model = model_of(parse_quote! {
            #[from(std::io::Error(kind: error.kind()), std::fmt::Error)]
            struct T { kind: std::io::ErrorKind, inner: ohno::OhnoCore }
        });

        assert_eq!(model.conversions.len(), 2);
        assert_eq!(model.conversions[0].initializers().len(), 1);
    }

    #[test]
    fn a_missing_core_is_reported() {
        assert!(
            faults_of(parse_quote!(
                struct T {
                    path: String,
                }
            ))
            .contains("No field holds the OhnoCore")
        );
    }

    #[test]
    fn several_unmarked_cores_are_reported() {
        let faults = faults_of(parse_quote! {
            struct T { first: ohno::OhnoCore, second: ohno::OhnoCore }
        });

        assert!(faults.contains("Several fields hold an OhnoCore"), "{faults}");
    }

    #[test]
    fn a_second_marked_field_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T { #[error] first: ohno::OhnoCore, #[error] second: ohno::OhnoCore }
        });

        assert_eq!(faults.matches("compile_error").count(), 1, "{faults}");
        assert!(faults.contains("Multiple fields marked"), "{faults}");
    }

    #[test]
    fn a_field_marked_twice_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T { #[error] #[error] inner: ohno::OhnoCore }
        });

        assert_eq!(faults.matches("compile_error").count(), 1, "{faults}");
        assert!(faults.contains("Duplicate `#[error]`"), "{faults}");
    }

    #[test]
    fn a_marker_beside_the_generated_one_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T {
                #[error]
                #[doc = " ohno::generated-core@7f3d9c2a"]
                inner: ohno::OhnoCore,
            }
        });

        assert!(faults.contains("Multiple fields marked"), "{faults}");
    }

    #[test]
    fn a_marker_on_a_sibling_of_the_generated_one_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T {
                #[error]
                carried: ohno::OhnoCore,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert!(faults.contains("Multiple fields marked"), "{faults}");
    }

    #[test]
    fn a_bad_template_and_a_bad_core_are_reported_together() {
        let faults = faults_of(parse_quote! {
            #[display("bad path: {pth}")]
            struct T { path: String }
        });

        assert!(faults.contains("unknown field `pth`"), "{faults}");
        assert!(faults.contains("No field holds the OhnoCore"), "{faults}");
    }

    #[test]
    fn a_bad_conversion_key_is_reported() {
        let faults = faults_of(parse_quote! {
            #[from(std::io::Error(missing: 1))]
            struct T { kind: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("unknown field `missing`"), "{faults}");
    }

    #[test]
    fn a_bad_conversion_does_not_stop_the_others() {
        let (model, faults) = validate_of(parse_quote! {
            #[from(std::io::Error(missing: 1), std::fmt::Error)]
            struct T { kind: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("unknown field `missing`"), "{faults}");
        assert_eq!(model.expect("a model").conversions.len(), 1);
    }
}
