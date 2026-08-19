// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::derive_error::display::argument::*;
use syn::Expr;

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn root_name(expr: &Expr) -> Option<String> {
        root(expr).field_name().map(ToOwned::to_owned)
    }

    #[test]
    fn a_bare_name_is_its_own_root() {
        assert_eq!(root_name(&parse_quote!(count)).as_deref(), Some("count"));
    }

    #[test]
    fn a_raw_identifier_keeps_its_prefix() {
        assert_eq!(root_name(&parse_quote!(r#type)).as_deref(), Some("r#type"));
    }

    #[test]
    fn every_leftmost_form_reaches_the_same_root() {
        let expressions: Vec<Expr> = vec![
            parse_quote!(count.value),
            parse_quote!(count.value()),
            parse_quote!(count[0]),
            parse_quote!(count * 2),
            parse_quote!(count as u64),
            parse_quote!(count.await),
            parse_quote!(count?),
            parse_quote!(count..10),
            parse_quote!(count.0.1.message()),
        ];

        for expr in &expressions {
            assert_eq!(root_name(expr).as_deref(), Some("count"), "{}", quote::quote!(#expr));
        }
    }

    #[test]
    fn an_integer_literal_names_a_tuple_field() {
        assert_eq!(root_name(&parse_quote!(0)).as_deref(), Some("0"));
        assert_eq!(root_name(&parse_quote!(1.abs())).as_deref(), Some("1"));
    }

    #[test]
    fn a_float_literal_is_a_nested_tuple_access() {
        assert_eq!(root_name(&parse_quote!(0.1)).as_deref(), Some("0"));
        assert_eq!(root_name(&parse_quote!(2.0)).as_deref(), Some("2"));
    }

    #[test]
    fn a_suffixed_numeric_literal_is_not_a_tuple_field() {
        // A suffix is not part of a tuple index, and `base10_digits` drops it, so accepting one
        // would validate against a field the expansion never reaches: `self.0u8` does not parse.
        let expressions: Vec<Expr> = vec![
            parse_quote!(0u8),
            parse_quote!(1usize.abs()),
            parse_quote!(0.1f32),
            parse_quote!(2.0f64),
        ];

        for expr in &expressions {
            let rendered = quote::quote!(#expr).to_string();
            assert!(matches!(root(expr), Root::Unsupported), "expected unsupported: {rendered}");
        }
    }

    #[test]
    fn a_bare_call_is_a_method_of_self() {
        assert!(matches!(root(&parse_quote!(describe())), Root::Method));
        assert!(matches!(root(&parse_quote!(describe().len())), Root::Method));
        assert_eq!(root_name(&parse_quote!(describe())), None);
    }

    #[test]
    fn self_is_reported_separately() {
        assert!(matches!(root(&parse_quote!(self)), Root::SelfKeyword(_)));
        assert!(matches!(root(&parse_quote!(self.path.display())), Root::SelfKeyword(_)));
        assert_eq!(root_name(&parse_quote!(self)), None);
    }

    #[test]
    fn a_qualified_path_is_not_a_bare_name() {
        // `Path::get_ident` returns `None` for a qualified path, so `<T>::VALUE` never reads as the
        // bare name `VALUE`.
        assert!(matches!(root(&parse_quote!(<T>::VALUE)), Root::Unsupported));
    }

    #[test]
    fn a_root_that_cannot_follow_a_dot_is_unsupported() {
        let expressions: Vec<Expr> = vec![
            parse_quote!(Self::LABEL.len()),
            parse_quote!("prefix".len()),
            parse_quote!(std::mem::size_of::<u8>()),
            // A qualified path is not a bare name: `self.<T>::VALUE` does not parse.
            parse_quote!(<T>::VALUE),
            parse_quote!(<T as Trait>::VALUE),
            parse_quote!((count)),
            parse_quote!(-count),
            parse_quote!(..10),
            parse_quote!('c'),
        ];

        for expr in &expressions {
            let rendered = quote::quote!(#expr).to_string();
            assert!(matches!(root(expr), Root::Unsupported), "expected unsupported: {rendered}");
        }
    }
}
