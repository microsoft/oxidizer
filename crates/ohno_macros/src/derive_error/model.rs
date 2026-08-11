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

use super::ast::Style;
use super::parse::member_name;
use crate::diagnostics::Errors;
use crate::message::Message;

/// A validated error type.
pub(crate) struct Model {
    /// The type's name.
    pub(crate) ident: Ident,
    /// The type's generics, threaded through every generated item.
    pub(crate) generics: Generics,
    /// The fields, with the core singled out.
    pub(crate) shape: Shape,
    /// The `#[display(...)]` message, already lowered.
    pub(crate) message: Option<Message>,
    /// One entry per generated `From<T>`.
    pub(crate) conversions: Vec<Conversion>,
    /// Whether to generate `Debug`.
    pub(crate) debug: bool,
    /// Whether to generate `new` and `caused_by`.
    pub(crate) constructors: bool,
}

/// The fields in declaration order, split around the one holding the core.
///
/// Splitting rather than carrying an index into one list is what makes "exactly one core"
/// unrepresentable: an index can dangle, so every generator using it would need a branch for a core
/// that is not there. Declaration order is still recoverable, and `Style` and the members are read
/// from the same value, so they cannot disagree.
pub(crate) struct Shape {
    /// Whether the fields are named or positional.
    pub(crate) style: Style,
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
    /// Returns `None` when `core` is out of range, which is the last point at which that is
    /// representable.
    pub(crate) fn new(mut fields: Vec<ModelField>, core: usize, style: Style) -> Option<Self> {
        if core >= fields.len() {
            return None;
        }

        let after = fields.split_off(core + 1);
        let core = fields.pop()?;

        Some(Self {
            style,
            before: fields,
            core,
            after,
        })
    }

    /// The field holding the core.
    pub(crate) fn core(&self) -> &ModelField {
        &self.core
    }

    /// Every field, in declaration order. What `Debug` prints.
    pub(crate) fn all(&self) -> impl Iterator<Item = &ModelField> {
        self.before.iter().chain(std::iter::once(&self.core)).chain(self.after.iter())
    }

    /// Every field but the core, in declaration order.
    ///
    /// What the constructors take, and what a conversion initializes.
    pub(crate) fn data(&self) -> impl Iterator<Item = &ModelField> {
        self.before.iter().chain(self.after.iter())
    }
}
/// One field of a validated error type.
pub(crate) struct ModelField {
    /// How the field is written in an expression: `path`, or `0`.
    pub(crate) member: Member,
    /// How the field is bound as a constructor parameter.
    ///
    /// The field's own name for a named struct, and `param_0`, `param_1`, … by field index for a
    /// tuple one.
    pub(crate) binding: Ident,
    /// The field's declared type.
    pub(crate) ty: Type,
}

impl ModelField {
    /// Builds a field, deriving its constructor binding from its member.
    pub(crate) fn new(member: Member, ty: Type) -> Self {
        let binding = match &member {
            Member::Named(ident) => ident.clone(),
            Member::Unnamed(index) => format_ident!("param_{}", index.index, span = Span::call_site()),
        };

        Self { member, binding, ty }
    }
}

/// One generated `From<T>`.
pub(crate) struct Conversion {
    /// The type the conversion converts from.
    pub(crate) source: Type,
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
    /// recorded.
    pub(crate) fn new(shape: &Shape, source: Type, overrides: &[(Member, Expr)], errors: &mut Errors) -> Option<Self> {
        let mut initializers: Vec<Expr> = Vec::new();

        for field in shape.data() {
            let found = overrides.iter().find(|(key, _)| member_name(key) == member_name(&field.member));
            initializers.push(match found {
                Some((_, value)) => value.clone(),
                None => syn::parse_quote!(::core::default::Default::default()),
            });
        }

        let mut usable = true;
        for (key, _) in overrides {
            if !shape.data().any(|field| member_name(&field.member) == member_name(key)) {
                errors.add(key, unknown_key(shape, key));
                usable = false;
            }
        }

        usable.then_some(Self { source, initializers })
    }

    /// The initializers, in the order [`Shape::data`] yields its fields.
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

    if member_name(&shape.core().member) == name {
        return format!("`#[from(...)]` cannot initialize `{name}`, which holds the OhnoCore and is built from the source error");
    }

    let available = shape
        .data()
        .map(|field| format!("`{}`", member_name(&field.member)))
        .collect::<Vec<_>>()
        .join(", ");

    format!("unknown field `{name}` in `#[from(...)]`, available fields: {available}")
}

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
