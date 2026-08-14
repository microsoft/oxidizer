// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rooting a `#[display(...)]` positional argument in a field of `self`.
//!
//! An argument is written without the `self.` prefix, so the prefix is applied here. An argument is
//! scoped by its leftmost term, and that term is the one that has to name a field, or call a method
//! of `self`: `count * 2` is rooted at `count`, `t.0.message()` at `t`, and `describe()` calls a
//! method.

use syn::{Expr, Lit};

/// What an argument is rooted in.
pub(crate) enum Root<'a> {
    /// `self`, written explicitly. Reported separately, because it would expand to `self.self`.
    SelfKeyword(&'a Expr),
    /// A single-segment path, such as `count` or `r#type`.
    Name(String, &'a Expr),
    /// A numeric literal naming a tuple field.
    ///
    /// A float root is a nested tuple access: `0.1` lexes as one literal, and only its leading
    /// component names a field of `self`.
    Index(String, &'a Expr),
    /// A method of `self`, called without a prefix, such as `describe()`. It names no field.
    Method,
    /// Nothing that can legally follow `self.`.
    Unsupported,
}

impl Root<'_> {
    /// The name the root is looked up by, if it names a field at all.
    #[cfg(test)]
    fn field_name(&self) -> Option<&str> {
        match self {
            Self::Name(name, _) | Self::Index(name, _) => Some(name),
            Self::SelfKeyword(_) | Self::Method | Self::Unsupported => None,
        }
    }
}

/// The diagnostic for an argument written with a `self.` prefix.
pub(crate) const SELF_PREFIXED: &str = "`#[display(...)]` positional arguments are implicitly scoped to `self`, \
     so a field is referenced by its bare name, without a `self.` prefix";

/// The diagnostic for an argument that cannot follow `self.` at all.
pub(crate) const UNSUPPORTED_ROOT: &str = "`#[display(...)]` positional arguments are implicitly scoped to `self`, \
     so each argument must be rooted in a field or method of `self`";

/// Finds the leftmost term of `expr`.
///
/// Field access, method calls, indexing, binary operators, casts, `await`, `?` and ranges all keep
/// a term in leftmost position, which is where the prefix lands. A call in that position is a
/// method of `self` rather than a field.
pub(crate) fn root(expr: &Expr) -> Root<'_> {
    match expr {
        // A qualified path such as `<T>::VALUE` is rejected here too: `Path::get_ident` returns
        // `None` for it, and `self.<T>::VALUE` would not parse anyway.
        Expr::Path(path) => {
            let Some(ident) = path.path.get_ident() else {
                return Root::Unsupported;
            };

            if ident == "self" {
                Root::SelfKeyword(expr)
            } else {
                Root::Name(ident.to_string(), expr)
            }
        }
        Expr::Lit(literal) => match &literal.lit {
            Lit::Int(value) => Root::Index(value.base10_digits().to_owned(), expr),
            // `0.1` is a nested tuple access, and only its leading component names a field.
            Lit::Float(value) => value
                .base10_digits()
                .split_once('.')
                .map_or(Root::Unsupported, |(leading, _)| Root::Index(leading.to_owned(), expr)),
            _ => Root::Unsupported,
        },
        Expr::Field(inner) => root(&inner.base),
        Expr::MethodCall(inner) => root(&inner.receiver),
        // A call rooted in a bare name is a method of `self`: `describe()` becomes
        // `self.describe()`, which names no field.
        Expr::Call(inner) => match root(&inner.func) {
            Root::Name(..) => Root::Method,
            other => other,
        },
        Expr::Index(inner) => root(&inner.expr),
        Expr::Binary(inner) => root(&inner.left),
        Expr::Cast(inner) => root(&inner.expr),
        Expr::Await(inner) => root(&inner.base),
        Expr::Try(inner) => root(&inner.expr),
        Expr::Range(inner) => inner.start.as_deref().map_or(Root::Unsupported, root),
        _ => Root::Unsupported,
    }
}

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
