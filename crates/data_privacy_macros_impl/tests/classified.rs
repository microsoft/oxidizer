// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy_macros_impl::classified::classified;
use insta::assert_snapshot;
use quote::quote;

#[test]
fn test_classified_impl_empty_args() {
    let attr_args = quote! {};
    let input = quote! {
            pub struct EmailAddress(String);
        };

    let result = classified(attr_args, input);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("classified attribute requires a taxonomy and data class name")
    );
}

#[test]
fn test_classified_impl_random_args() {
    let attr_args = quote! {Foo::Bar Baz};
    let input = quote! {
            struct EmailAddress(String);
        };

    let result = classified(attr_args, input);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("unexpected token"));
}

#[test]
fn test_classified_impl_named_fields() {
    let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
    let input = quote! {
            struct EmailAddress { x : String }
        };

    let result = classified(attr_args, input);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Named fields aren't supported"));
}

#[test]
fn test_classified_impl_no_fields() {
    let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
    let input = quote! {
            struct EmailAddress;
        };

    let result = classified(attr_args, input);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Unit structs aren't supported"));
}

#[test]
fn test_classified_impl_too_many_fields() {
    let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
    let input = quote! {
            struct EmailAddress(String, i32);
        };

    let result = classified(attr_args, input);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Tuple struct must have exactly one field"));
}

#[test]
fn test_success() {
    let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
    let input = quote! {
            struct EmailAddress(String);
        };

    let result = classified(attr_args, input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}
