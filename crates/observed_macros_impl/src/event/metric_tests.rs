// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[test]
#[should_panic(expected = "instrument value types are validated before code generation")]
fn code_generation_checks_its_numeric_validation_invariant() {
    let item: syn::ItemStruct = syn::parse_quote! {
        #[counter(value)]
        struct Example { #[unredacted] value: u64 }
    };
    let args = EventArgs {
        name: "example".into(),
        disabled: false,
    };
    let mut definition = parse_event_def(&item.ident, &item.generics, &item.attrs, &item.fields, &args).unwrap();
    definition.fields[0].ty = syn::parse_quote!(bool);
    let _tokens = generate_metric_items(&definition);
}
