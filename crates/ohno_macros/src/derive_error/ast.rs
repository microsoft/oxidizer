// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The input with the crate's own attributes decoded.
//!
//! `Ast` records what the user wrote. Whether what they wrote is allowed is decided by
//! [`validate`](super::validate), which reports the derive's own rule violations.

use syn::{Attribute, Expr, Generics, Ident, LitStr, Member, Type};

/// Whether the struct's fields are named or positional.
///
/// A unit struct never reaches `Ast`: the derive rejects it, because there is no room for a core.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Style {
    /// `struct T { a: A }`
    Named,
    /// `struct T(A);`
    Tuple,
}

/// What the struct says.
pub(crate) struct Ast {
    /// The type's name.
    pub(crate) ident: Ident,
    /// The type's generics, passed through untouched.
    pub(crate) generics: Generics,
    /// Whether the fields are named or positional.
    pub(crate) style: Style,
    /// The fields, in declaration order.
    pub(crate) fields: Vec<AstField>,
    /// The `#[display(...)]` message. `None` when absent, or when it failed to decode.
    pub(crate) display: Option<DisplayAttr>,
    /// One entry per type listed across every `#[from(...)]`.
    pub(crate) conversions: Vec<FromAttr>,
    /// Whether `#[no_debug]` was written.
    pub(crate) no_debug: bool,
    /// Whether `#[no_constructors]` was written.
    pub(crate) no_constructors: bool,
}

/// One field, with the markers that may designate it as the error field.
pub(crate) struct AstField {
    /// How the field is written in an expression: `path`, or `0`.
    pub(crate) member: Member,
    /// The field's declared type.
    pub(crate) ty: Type,
    /// Every hand-written `#[error]` on this field, in order.
    ///
    /// A `Vec` rather than an `Option`, so a field marked twice is representable and therefore
    /// reportable.
    pub(crate) marks: Vec<Attribute>,
    /// Whether the field carries the reserved marker `#[ohno::error]` writes.
    pub(crate) generated: bool,
}

impl AstField {
    /// Whether the field may be named in a `#[display(...)]` template.
    ///
    /// Every field the user wrote is referenceable. The core added by `#[ohno::error]` is not:
    /// printing it would print the error's own chain, and naming it would point at a field that is
    /// not in the user's code.
    pub(crate) fn is_referenceable(&self) -> bool {
        !self.generated
    }
}

/// A decoded `#[display(...)]`.
pub(crate) struct DisplayAttr {
    /// The template literal, kept whole so a template fault can point at it.
    pub(crate) template: LitStr,
    /// The positional arguments.
    pub(crate) arguments: Vec<Expr>,
}

/// One type listed in a `#[from(...)]`, with the field expressions written for it.
pub(crate) struct FromAttr {
    /// The type the generated `From` converts from.
    pub(crate) source: Type,
    /// The field expressions, keyed as the user wrote them.
    pub(crate) overrides: Vec<FromOverride>,
}

/// One `key: expression` pair inside a `#[from(...)]` entry.
pub(crate) struct FromOverride {
    /// The field the expression initializes, as the user keyed it.
    pub(crate) key: Member,
    /// The expression to initialize it with.
    pub(crate) value: Expr,
}
