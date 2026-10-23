// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::parse_quote;

use super::*;

#[test]
#[should_panic(expected = "instrument value types are validated before code generation")]
fn code_generation_checks_its_numeric_validation_invariant() {
    let item: ItemStruct = parse_quote! {
        #[counter(value)]
        struct Example { #[unredacted] value: u64 }
    };
    let args = EventArgs {
        name: "example".into(),
        disabled: false,
    };
    let mut definition = parse_event_def(&item.ident, &item.generics, &item.attrs, &item.fields, &args).unwrap();
    let invalid_type: Type = parse_quote!(bool);
    definition.fields[0].ty = &invalid_type;
    let _tokens = generate_field_descriptor(&definition.fields[0], false);
}
