// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A validated error type, ready to generate from.
//!
//! `Model` holds resolved values, not syntax. Generation reads it and cannot fail, because a
//! `Model` that would make generation fail cannot be built: the only way to obtain one is through
//! [`validate`](super::validate).

use proc_macro2::Span;
use quote::format_ident;
use syn::{Expr, Generics, Ident, Member, Type};

use super::ast::{FromOverride, Style};
use super::member_name;
use crate::diagnostics::Errors;
use crate::message::Message;

/// A validated error type.
#[derive(Debug)]
pub(crate) struct Model {
    /// The type's name.
    pub ident: Ident,
    /// The type's generics, threaded through every generated item.
    pub generics: Generics,
    /// The fields, with the core singled out.
    pub shape: Shape,
    /// The `#[display(...)]` message, already lowered.
    pub message: Option<Message>,
    /// One entry per generated `From<T>`.
    pub conversions: Vec<Conversion>,
    /// Whether to generate `Debug`.
    pub debug: bool,
    /// Whether to generate `new` and `caused_by`.
    pub constructors: bool,
}

/// The fields in declaration order, split around the one holding the core.
///
/// Splitting rather than carrying an index into one list is what makes "exactly one core"
/// unrepresentable: an index can dangle, so every generator using it would need a branch for a core
/// that is not there. Declaration order is still recoverable, and `Style` and the members are read
/// from the same value, so they cannot disagree.
#[derive(Debug)]
pub(crate) struct Shape {
    /// Whether the fields are named or positional.
    pub style: Style,
    /// The fields declared before the core.
    before: Vec<ModelField>,
    /// The field holding the `OhnoCore`.
    core: ModelField,
    /// The fields declared after the core.
    after: Vec<ModelField>,
}

impl Shape {
    /// Builds a shape from the fields in declaration order and the index of the core among them.
    ///
    /// # Panics
    ///
    /// Panics when `core` does not index `fields`. [`validate`](super::validate::validate) derives
    /// the index from the very field list it then maps into `fields`, so the two cannot disagree.
    #[must_use]
    pub(crate) fn new(mut fields: Vec<ModelField>, core: usize, style: Style) -> Self {
        let after = fields.split_off(core + 1);
        let core = fields.pop().expect("`core` indexes `fields`, so the split leaves it in place");

        Self {
            style,
            before: fields,
            core,
            after,
        }
    }

    /// The field holding the core.
    #[must_use]
    pub(crate) fn core(&self) -> &ModelField {
        &self.core
    }

    /// Every field, in declaration order, each with its [`Position`]. This is what generation walks.
    ///
    /// Pairing each field with its position here is what keeps "which field holds the core" a fact
    /// of the shape. A generator that recovered it by comparing members would be re-deciding, at
    /// every use, something the split around the core already settled.
    pub(crate) fn positions(&self) -> impl Iterator<Item = (&ModelField, Position)> {
        let after = self.before.len();

        self.before
            .iter()
            .enumerate()
            .map(|(index, field)| (field, Position::Data(index)))
            .chain(std::iter::once((&self.core, Position::Core)))
            .chain(
                self.after
                    .iter()
                    .enumerate()
                    .map(move |(index, field)| (field, Position::Data(after + index))),
            )
    }

    /// Every field, in declaration order. What `Debug` prints.
    pub(crate) fn all(&self) -> impl Iterator<Item = &ModelField> {
        self.positions().map(|(field, _)| field)
    }

    /// Every field but the core, in declaration order.
    ///
    /// What the constructors take, and what a conversion initializes.
    pub(crate) fn data(&self) -> impl Iterator<Item = &ModelField> {
        self.before.iter().chain(self.after.iter())
    }
}
/// Where a field sits in a [`Shape`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum Position {
    /// The field holding the `OhnoCore`.
    Core,
    /// A field other than the core, numbered as [`Shape::data`] yields it.
    Data(usize),
}

/// One field of a validated error type.
#[derive(Debug)]
pub(crate) struct ModelField {
    /// How the field is written in an expression: `path`, or `0`.
    pub member: Member,
    /// How the field is bound as a constructor parameter.
    ///
    /// The field's own name for a named struct, and `param_0`, `param_1`, … by field index for a
    /// tuple one.
    pub binding: Ident,
    /// The field's declared type.
    pub ty: Type,
}

impl ModelField {
    /// Builds a field, deriving its constructor binding from its member.
    #[must_use]
    pub(crate) fn new(member: Member, ty: Type) -> Self {
        let binding = match &member {
            Member::Named(ident) => ident.clone(),
            Member::Unnamed(index) => format_ident!("param_{}", index.index, span = Span::call_site()),
        };

        Self { member, binding, ty }
    }
}

/// One generated `From<T>`.
#[derive(Debug)]
pub(crate) struct Conversion {
    /// The type the conversion converts from.
    pub source: Type,
    /// One initializer per non-core field, aligned with [`Shape::data`].
    initializers: Vec<Expr>,
}

impl Conversion {
    /// Distributes `overrides` over the shape's non-core fields, defaulting the rest.
    ///
    /// The alignment between the initializers and the field list is a relation between two values,
    /// which Rust cannot hold in a struct shape without an encoding heavier than the check it
    /// replaces. It is established here instead, once, where the field list is in hand.
    ///
    /// Returns `None` when an override names no non-core field, in which case a fault has been
    /// recorded. The keys are checked before anything is built, so a rejected conversion costs no
    /// initializer that is then thrown away.
    pub(crate) fn new(shape: &Shape, source: Type, overrides: &[FromOverride], errors: &mut Errors) -> Option<Self> {
        let mut usable = true;
        for entry in overrides {
            if !shape.data().any(|field| field.member == entry.key) {
                errors.add(&entry.key, unknown_key(shape, &entry.key));
                usable = false;
            }
        }

        usable.then(|| {
            let initializers = shape
                .data()
                .map(|field| {
                    overrides.iter().find(|entry| entry.key == field.member).map_or_else(
                        || syn::parse_quote!(::core::default::Default::default()),
                        |entry| entry.value.clone(),
                    )
                })
                .collect();

            Self { source, initializers }
        })
    }

    /// The initializers, in the order [`Shape::data`] yields its fields.
    #[must_use]
    pub(crate) fn initializers(&self) -> &[Expr] {
        &self.initializers
    }
}

/// The diagnostic for a `#[from(...)]` key that names no field a conversion can initialize.
fn unknown_key(shape: &Shape, key: &Member) -> String {
    let name = member_name(key);

    if shape.style == Style::Tuple && matches!(key, Member::Named(_)) {
        return format!("`#[from(...)]` field keys for a tuple struct are field indexes, not names, so `{name}:` names no field");
    }

    if shape.core().member == *key {
        return format!("`#[from(...)]` cannot initialize `{name}`, which holds the OhnoCore and is built from the source error");
    }

    let available = shape
        .data()
        .map(|field| format!("`{}`", member_name(&field.member)))
        .collect::<Vec<_>>()
        .join(", ");

    format!("unknown field `{name}` in `#[from(...)]`, available fields: {available}")
}
