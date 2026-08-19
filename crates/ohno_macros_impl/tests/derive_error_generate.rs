// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::derive_error::ast::Style;
use ohno_macros_impl::derive_error::generate::*;
use ohno_macros_impl::derive_error::model::{Model, Shape};
use quote::quote;
use syn::Member;

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
// `unsupported operation: `open` not available when isolation is enabled`
#[cfg(all(test, not(miri)))]
mod tests {
    use ohno_macros_impl::derive_error::model::{Conversion, ModelField};
    use ohno_macros_impl::diagnostics::Errors;
    use ohno_macros_impl::message::Message;
    use quote::format_ident;
    use syn::{Ident, parse_quote};

    use super::*;

    /// Builds a `Model` by hand, without going through parse or validate.
    ///
    /// A generator reads only a `Model`, so its tests build one directly. That keeps the snapshots
    /// independent of attribute syntax, and lets a shape no real input produces still be covered.
    struct Builder {
        ident: Ident,
        generics: syn::Generics,
        fields: Vec<ModelField>,
        core: usize,
        style: Style,
        message: Option<Message>,
        conversions: Vec<(syn::Type, Vec<(Member, syn::Expr)>)>,
        debug: bool,
        constructors: bool,
    }

    impl Builder {
        fn named(names: &[&str], core: usize) -> Self {
            let fields = names
                .iter()
                .map(|name| ModelField::new(Member::Named(format_ident!("{name}")), parse_quote!(String)))
                .collect();
            Self::new(fields, core, Style::Named)
        }

        fn tuple(count: usize, core: usize) -> Self {
            let fields = (0..count)
                .map(|index| ModelField::new(Member::Unnamed(syn::Index::from(index)), parse_quote!(String)))
                .collect();
            Self::new(fields, core, Style::Tuple)
        }

        fn new(fields: Vec<ModelField>, core: usize, style: Style) -> Self {
            Self {
                ident: format_ident!("MyError"),
                generics: syn::Generics::default(),
                fields,
                core,
                style,
                message: None,
                conversions: Vec::new(),
                debug: true,
                constructors: true,
            }
        }

        fn generics(mut self, generics: syn::Generics, where_clause: Option<syn::WhereClause>) -> Self {
            self.generics = generics;
            self.generics.where_clause = where_clause;
            self
        }

        fn message(mut self, message: Message) -> Self {
            self.message = Some(message);
            self
        }

        fn conversion(mut self, source: syn::Type, overrides: Vec<(Member, syn::Expr)>) -> Self {
            self.conversions.push((source, overrides));
            self
        }

        fn without_debug(mut self) -> Self {
            self.debug = false;
            self
        }

        fn without_constructors(mut self) -> Self {
            self.constructors = false;
            self
        }

        fn build(self) -> Model {
            let shape = Shape::new(self.fields, self.core, self.style).expect("a shape");
            let mut errors = Errors::default();
            let conversions = self
                .conversions
                .into_iter()
                .map(|(source, overrides)| Conversion::new(&shape, source, &overrides, &mut errors).expect("a conversion"))
                .collect();
            assert!(errors.is_empty());

            Model {
                ident: self.ident,
                generics: self.generics,
                shape,
                message: self.message,
                conversions,
                debug: self.debug,
                constructors: self.constructors,
            }
        }
    }

    /// Generates from `model` and pretty-prints the result.
    fn rendered(model: &Model) -> String {
        let tokens = generate(model);
        let file: syn::File = syn::parse2(tokens).expect("the generated items parse as a file");
        prettyplease::unparse(&file)
    }

    #[test]
    fn a_named_struct_generates_every_item() {
        insta::assert_snapshot!(rendered(&Builder::named(&["path", "inner"], 1).build()));
    }

    #[test]
    fn a_tuple_struct_generates_positional_items() {
        insta::assert_snapshot!(rendered(&Builder::tuple(3, 2).build()));
    }

    #[test]
    fn a_core_in_the_middle_keeps_declaration_order() {
        insta::assert_snapshot!(rendered(&Builder::named(&["before", "core", "after"], 1).build()));
    }

    #[test]
    fn a_single_field_struct_takes_no_constructor_parameters() {
        insta::assert_snapshot!(rendered(&Builder::named(&["inner"], 0).build()));
    }

    #[test]
    fn a_message_overrides_the_default() {
        let message = Message::Formatted {
            template: "failed for {}".to_owned(),
            arguments: vec![quote!(&(self.path))],
        };
        insta::assert_snapshot!(rendered(&Builder::named(&["path", "inner"], 1).message(message).build()));
    }

    #[test]
    fn generics_thread_through_every_impl() {
        let generics: syn::Generics = parse_quote!(<'a, T: Clone>);
        let where_clause: syn::WhereClause = parse_quote!(where T: Send);
        let model = Builder::named(&["path", "inner"], 1).generics(generics, Some(where_clause)).build();
        insta::assert_snapshot!(rendered(&model));
    }

    #[test]
    fn conversions_initialize_every_non_core_field() {
        let model = Builder::named(&["kind", "inner"], 1)
            .conversion(
                parse_quote!(std::io::Error),
                vec![(Member::Named(format_ident!("kind")), parse_quote!(error.kind()))],
            )
            .conversion(parse_quote!(std::fmt::Error), Vec::new())
            .build();
        insta::assert_snapshot!(rendered(&model));
    }

    #[test]
    fn the_suppressing_flags_remove_their_items() {
        insta::assert_snapshot!(rendered(
            &Builder::named(&["path", "inner"], 1).without_debug().without_constructors().build()
        ));
    }
}
