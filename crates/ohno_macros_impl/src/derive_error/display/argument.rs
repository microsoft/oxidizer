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
#[derive(Debug)]
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
            // A suffix is not part of a tuple index, so a suffixed literal names no field:
            // `self.0u8` does not parse.
            Lit::Int(value) if value.suffix().is_empty() => Root::Index(value.base10_digits().to_owned(), expr),
            // `0.1` is a nested tuple access, and only its leading component names a field.
            Lit::Float(value) if value.suffix().is_empty() => value
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
