// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::parse_quote;

use super::*;

#[test]
fn nov6_parsed_qualified_self_targets_cannot_be_unqualified_single_segments() {
    let message = "qualified impl targets are not supported; apply the impl in the service's declaration module";
    for source in ["impl<T> <T>::Service {}", "impl<T> <T as Trait>::Service {}"] {
        let item: ItemImpl = syn::parse_str(source).unwrap();
        let syn::Type::Path(path) = &*item.self_ty else {
            panic!("the fixture must parse as a type path");
        };
        assert!(path.qself.is_some());
        assert!(path.path.leading_colon.is_some() || path.path.segments.len() > 1);
        assert_eq!(extract_struct_segment(&item).unwrap_err().to_string(), message);
    }
    for source in ["impl ::Service {}", "impl module::Service {}"] {
        let item = syn::parse_str(source).unwrap();
        assert_eq!(extract_struct_segment(&item).unwrap_err().to_string(), message);
    }
    for source in ["impl Service {}", "impl Service<u32> {}"] {
        let item = syn::parse_str(source).unwrap();
        assert_eq!(extract_struct_segment(&item).unwrap().ident, "Service");
    }
}

#[test]
fn gamma_survivor_first_free_lifetime_is_forwarded_to_all_nested_arguments() {
    let signature = parse_quote!(fn method<'mock, 'mock1, T, U>(&self, input: Option<&str>, second: (&str, Option<&'mock str>)));
    let actual = add_explicit_lifetimes(&signature);
    let expected: syn::Signature = parse_quote!(
        fn method<'mock, 'mock1, 'mock2, T, U>(&self, input: Option<&'mock2 str>, second: (&'mock2 str, Option<&'mock str>))
    );
    assert_eq!(actual.to_token_stream().to_string(), expected.to_token_stream().to_string());
}

#[test]
fn gamma_survivor_qualified_associated_types_are_not_bare_wrapper_names() {
    let ty = parse_quote!(<T as Self>::Output);
    assert!(!type_contains_bare_self(&ty));
    let ty = parse_quote!(<T as Trait>::Service);
    assert!(!type_contains_concrete_service_name(&ty, &parse_quote!(Service)));
    assert!(trait_path_contains_wrapper_sensitive_type(
        &parse_quote!(First<u32>::Last<Service>),
        &parse_quote!(Service)
    ));
}

#[cfg(feature = "mockall")]
#[test]
fn gamma_survivor_higher_ranked_detection_recurses_without_inventing_elision() {
    for ty in [
        parse_quote!(fn(&'static str)),
        parse_quote!(for<'a> fn(&'a str)),
        parse_quote!(dyn for<'a> Fn(&'a str)),
    ] {
        assert!(!type_contains_implicit_higher_ranked_elision(&ty));
    }
    for ty in [
        parse_quote!(fn(fn(&str))),
        parse_quote!(fn(&'static Option<&str>)),
        parse_quote!(dyn Iterator<Item = fn(&str)>),
    ] {
        assert!(type_contains_implicit_higher_ranked_elision(&ty));
    }
}
