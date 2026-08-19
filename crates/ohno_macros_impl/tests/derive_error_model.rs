// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::derive_error::ast::Style;
use ohno_macros_impl::derive_error::model::*;
use ohno_macros_impl::derive_error::parse::member_name;
use ohno_macros_impl::diagnostics::Errors;
use quote::format_ident;
use syn::{Expr, Member};

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn field(name: &str) -> ModelField {
        ModelField::new(Member::Named(format_ident!("{name}")), parse_quote!(u32))
    }

    fn tuple_field(index: u32) -> ModelField {
        ModelField::new(Member::Unnamed(syn::Index::from(index as usize)), parse_quote!(u32))
    }

    fn names(fields: impl Iterator<Item = &'static str>) -> Vec<String> {
        fields.map(ToOwned::to_owned).collect()
    }

    fn shape_of(fields: Vec<ModelField>, core: usize) -> Shape {
        Shape::new(fields, core, Style::Named).expect("a shape")
    }

    fn rendered(expr: &Expr) -> String {
        quote::quote!(#expr).to_string()
    }

    #[test]
    fn a_shape_keeps_declaration_order() {
        let shape = shape_of(vec![field("a"), field("core"), field("b")], 1);

        assert_eq!(
            shape.all().map(|f| member_name(&f.member)).collect::<Vec<_>>(),
            names(["a", "core", "b"].into_iter())
        );
        assert_eq!(
            shape.data().map(|f| member_name(&f.member)).collect::<Vec<_>>(),
            names(["a", "b"].into_iter())
        );
        assert_eq!(member_name(&shape.core().member), "core");
    }

    #[test]
    fn a_core_at_either_end_still_works() {
        let first = shape_of(vec![field("core"), field("b")], 0);
        assert_eq!(member_name(&first.core().member), "core");
        assert_eq!(first.data().count(), 1);

        let last = shape_of(vec![field("a"), field("core")], 1);
        assert_eq!(member_name(&last.core().member), "core");
        assert_eq!(last.data().count(), 1);
    }

    #[test]
    fn a_single_field_shape_has_no_data() {
        let shape = shape_of(vec![field("core")], 0);
        assert_eq!(shape.data().count(), 0);
        assert_eq!(shape.all().count(), 1);
    }

    #[test]
    fn an_out_of_range_core_is_refused() {
        assert!(Shape::new(vec![field("a")], 1, Style::Named).is_none());
        assert!(Shape::new(Vec::new(), 0, Style::Named).is_none());
    }

    #[test]
    fn a_named_field_binds_under_its_own_name() {
        assert_eq!(field("path").binding.to_string(), "path");
    }

    #[test]
    fn a_tuple_field_binds_under_its_index() {
        assert_eq!(tuple_field(2).binding.to_string(), "param_2");
    }

    #[test]
    fn a_conversion_defaults_every_field_it_is_not_given() {
        let shape = shape_of(vec![field("a"), field("core"), field("b")], 1);
        let mut errors = Errors::default();
        let conversion = Conversion::new(&shape, parse_quote!(std::io::Error), &[], &mut errors).expect("a conversion");

        assert!(errors.is_empty());
        assert_eq!(conversion.initializers().len(), 2);
        for initializer in conversion.initializers() {
            assert!(rendered(initializer).contains("Default"));
        }
    }

    #[test]
    fn a_conversion_places_an_override_on_its_own_field() {
        let shape = shape_of(vec![field("a"), field("core"), field("b")], 1);
        let overrides = vec![(Member::Named(format_ident!("b")), parse_quote!(error.kind()))];
        let mut errors = Errors::default();
        let conversion = Conversion::new(&shape, parse_quote!(std::io::Error), &overrides, &mut errors).expect("a conversion");

        assert!(rendered(&conversion.initializers()[0]).contains("Default"));
        assert!(rendered(&conversion.initializers()[1]).contains("kind"));
    }

    #[test]
    fn a_conversion_rejects_a_key_that_names_no_field() {
        let shape = shape_of(vec![field("a"), field("core")], 1);
        let overrides = vec![(Member::Named(format_ident!("missing")), parse_quote!(1))];
        let mut errors = Errors::default();

        assert!(Conversion::new(&shape, parse_quote!(std::io::Error), &overrides, &mut errors).is_none());
        assert!(errors.into_compile_error().to_string().contains("unknown field `missing`"));
    }

    #[test]
    fn a_conversion_rejects_a_key_naming_the_core() {
        let shape = shape_of(vec![field("a"), field("core")], 1);
        let overrides = vec![(Member::Named(format_ident!("core")), parse_quote!(1))];
        let mut errors = Errors::default();

        assert!(Conversion::new(&shape, parse_quote!(std::io::Error), &overrides, &mut errors).is_none());
        assert!(errors.into_compile_error().to_string().contains("holds the OhnoCore"));
    }

    #[test]
    fn a_conversion_rejects_a_named_key_on_a_tuple_struct() {
        let shape = Shape::new(vec![tuple_field(0), tuple_field(1)], 1, Style::Tuple).expect("a shape");
        let overrides = vec![(Member::Named(format_ident!("kind")), parse_quote!(1))];
        let mut errors = Errors::default();

        assert!(Conversion::new(&shape, parse_quote!(std::io::Error), &overrides, &mut errors).is_none());
        assert!(errors.into_compile_error().to_string().contains("field indexes, not names"));
    }

    #[test]
    fn a_conversion_survives_a_fault_recorded_elsewhere() {
        let shape = shape_of(vec![field("a"), field("core")], 1);
        let mut errors = Errors::default();
        errors.add(quote::quote!(unrelated), "an unrelated fault");

        assert!(Conversion::new(&shape, parse_quote!(std::io::Error), &[], &mut errors).is_some());
    }
}
