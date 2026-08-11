// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Ast`] into [`Model`].
//!
//! This is the only phase that reports rule violations. It runs every check it can, so an input
//! that breaks several rules is fixed in one pass rather than one per compile cycle.
//!
//! Accumulation runs across independent concerns. Core selection and the display message are
//! independent even though both concern fields: the set of referenceable fields is defined by the
//! reserved marker, not by which field selection picked, so a struct that marks two fields still
//! gets its template checked. A concern whose own input failed is skipped, not guessed at.

use syn::{Expr, Member};

use super::ast::{Ast, AstField};
use super::display::{self, Referenceable};
use super::model::{Conversion, Model, ModelField, Shape};
use super::parse::is_ohno_core;
#[cfg(test)]
use super::parse::member_name;
use crate::diagnostics::Errors;

/// The diagnostic for a second field carrying `#[error]`.
const MULTIPLE_MARKED: &str = "Multiple fields marked with `#[error]`. Mark only the field holding the OhnoCore";

/// The diagnostic for a second `#[error]` on one field.
const DUPLICATE_MARKER: &str = "Duplicate `#[error]` on the same field. Mark it once";

/// Applies the rules to `ast`.
///
/// Returns `None` when no `Model` can be built, in which case at least one fault has been recorded.
pub(crate) fn validate(ast: Ast, errors: &mut Errors) -> Option<Model> {
    let referenceable = Referenceable::new(&ast.fields);
    let message = ast
        .display
        .as_ref()
        .and_then(|display| display::lower(display, &referenceable, errors));

    let core = select_core(&ast, errors);

    let fields = ast
        .fields
        .iter()
        .map(|field| ModelField::new(field.member.clone(), field.ty.clone()))
        .collect();

    let shape = Shape::new(fields, core?, ast.style)?;

    let conversions = ast
        .conversions
        .iter()
        .filter_map(|conversion| {
            let overrides: Vec<(Member, Expr)> = conversion
                .overrides
                .iter()
                .map(|entry| (entry.key.clone(), entry.value.clone()))
                .collect();

            Conversion::new(&shape, conversion.source.clone(), &overrides, errors)
        })
        .collect();

    Some(Model {
        ident: ast.ident,
        generics: ast.generics,
        shape,
        message,
        conversions,
        debug: !ast.no_debug,
        constructors: !ast.no_constructors,
    })
}

/// Picks the field holding the core, reporting every way the choice can be spoiled.
///
/// The field is found in this order: the reserved marker `#[ohno::error]` writes, then `#[error]`,
/// then the single field whose type path ends in `OhnoCore`. A marked field's type is never
/// checked: the user has said which field it is, and `rustc` resolves the type, which is what lets
/// a core reached through an alias or a renamed import be used at all.
fn select_core(ast: &Ast, errors: &mut Errors) -> Option<usize> {
    report_duplicate_markers(&ast.fields, errors);

    if let Some(index) = ast.fields.iter().position(|field| field.generated) {
        return Some(index);
    }

    let mut marked = ast.fields.iter().enumerate().filter(|(_, field)| !field.marks.is_empty());
    if let Some((index, _)) = marked.next() {
        return Some(index);
    }

    let mut candidates = ast.fields.iter().enumerate().filter(|(_, field)| is_ohno_core(&field.ty));
    let first = candidates.next();

    match (first, candidates.next()) {
        (Some((index, _)), None) => Some(index),
        (Some(_), Some(_)) => {
            errors.add(
                &ast.ident,
                "Several fields hold an OhnoCore and none is marked. Mark the one holding the error representation with `#[error]`",
            );
            None
        }
        (None, _) => {
            errors.add(
                &ast.ident,
                "No field holds the OhnoCore. Declare one, mark it with `#[error]` if its type is spelled through an alias, \
                 or use `#[ohno::error]`, which adds the field itself",
            );
            None
        }
    }
}

/// Reports every marker beyond the first, whether it repeats on one field or spreads over several.
///
/// Reporting each extra marker rather than the first tells the user which one to delete.
fn report_duplicate_markers(fields: &[AstField], errors: &mut Errors) {
    let mut seen_marked_field = false;

    for field in fields {
        let mut marks = field.marks.iter();

        if let Some(first) = marks.next() {
            if seen_marked_field {
                errors.add(first, MULTIPLE_MARKED);
            }
            seen_marked_field = true;
        }

        for repeated in marks {
            errors.add(repeated, DUPLICATE_MARKER);
        }
    }

    for field in fields.iter().filter(|field| field.generated && !field.marks.is_empty()) {
        for mark in &field.marks {
            errors.add(mark, MULTIPLE_MARKED);
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;
    use crate::derive_error::parse;

    /// Validates `input`, returning the model and the faults.
    fn validate_of(input: syn::DeriveInput) -> (Option<Model>, String) {
        let mut errors = Errors::default();
        let ast = parse::parse(input, &mut errors).expect("the input parses");
        let model = validate(ast, &mut errors);
        (model, errors.into_compile_error().to_string())
    }

    fn model_of(input: syn::DeriveInput) -> Model {
        let (model, faults) = validate_of(input);
        assert!(faults.is_empty(), "unexpected faults: {faults}");
        model.expect("a model")
    }

    fn faults_of(input: syn::DeriveInput) -> String {
        let (_, faults) = validate_of(input);
        assert!(!faults.is_empty(), "expected a fault");
        faults
    }

    #[test]
    fn a_lone_ohno_core_field_is_the_core() {
        let model = model_of(parse_quote! {
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert_eq!(member_name(&model.shape.core().member), "inner");
        assert_eq!(model.shape.data().count(), 1);
    }

    #[test]
    fn a_marked_field_wins_over_its_type() {
        let model = model_of(parse_quote! {
            struct T { metadata: ohno::OhnoCore, #[error] main: Core }
        });

        assert_eq!(member_name(&model.shape.core().member), "main");
    }

    #[test]
    fn a_marked_field_is_not_type_checked() {
        let model = model_of(parse_quote! {
            struct T { path: String, #[error] inner: Core }
        });

        assert_eq!(member_name(&model.shape.core().member), "inner");
    }

    #[test]
    fn the_generated_marker_wins_over_a_declared_core() {
        let model = model_of(parse_quote! {
            struct T {
                carried: ohno::OhnoCore,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert_eq!(member_name(&model.shape.core().member), "ohno_core");
        assert_eq!(model.shape.data().count(), 1);
    }

    #[test]
    fn the_flags_invert_into_the_model() {
        let plain = model_of(parse_quote!(
            struct T {
                inner: ohno::OhnoCore,
            }
        ));
        assert!(plain.debug);
        assert!(plain.constructors);

        let suppressed = model_of(parse_quote! {
            #[no_debug]
            #[no_constructors]
            struct T { inner: ohno::OhnoCore }
        });
        assert!(!suppressed.debug);
        assert!(!suppressed.constructors);
    }

    #[test]
    fn conversions_reach_the_model() {
        let model = model_of(parse_quote! {
            #[from(std::io::Error(kind: error.kind()), std::fmt::Error)]
            struct T { kind: std::io::ErrorKind, inner: ohno::OhnoCore }
        });

        assert_eq!(model.conversions.len(), 2);
        assert_eq!(model.conversions[0].initializers().len(), 1);
    }

    #[test]
    fn a_missing_core_is_reported() {
        assert!(
            faults_of(parse_quote!(
                struct T {
                    path: String,
                }
            ))
            .contains("No field holds the OhnoCore")
        );
    }

    #[test]
    fn several_unmarked_cores_are_reported() {
        let faults = faults_of(parse_quote! {
            struct T { first: ohno::OhnoCore, second: ohno::OhnoCore }
        });

        assert!(faults.contains("Several fields hold an OhnoCore"), "{faults}");
    }

    #[test]
    fn a_second_marked_field_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T { #[error] first: ohno::OhnoCore, #[error] second: ohno::OhnoCore }
        });

        assert_eq!(faults.matches("compile_error").count(), 1, "{faults}");
        assert!(faults.contains("Multiple fields marked"), "{faults}");
    }

    #[test]
    fn a_field_marked_twice_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T { #[error] #[error] inner: ohno::OhnoCore }
        });

        assert_eq!(faults.matches("compile_error").count(), 1, "{faults}");
        assert!(faults.contains("Duplicate `#[error]`"), "{faults}");
    }

    #[test]
    fn a_marker_beside_the_generated_one_is_reported() {
        let faults = faults_of(parse_quote! {
            struct T {
                #[error]
                #[doc = " ohno::generated-core@7f3d9c2a"]
                inner: ohno::OhnoCore,
            }
        });

        assert!(faults.contains("Multiple fields marked"), "{faults}");
    }

    #[test]
    fn a_bad_template_and_a_bad_core_are_reported_together() {
        let faults = faults_of(parse_quote! {
            #[display("bad path: {pth}")]
            struct T { path: String }
        });

        assert!(faults.contains("unknown field `pth`"), "{faults}");
        assert!(faults.contains("No field holds the OhnoCore"), "{faults}");
    }

    #[test]
    fn a_bad_conversion_key_is_reported() {
        let faults = faults_of(parse_quote! {
            #[from(std::io::Error(missing: 1))]
            struct T { kind: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("unknown field `missing`"), "{faults}");
    }

    #[test]
    fn a_bad_conversion_does_not_stop_the_others() {
        let (model, faults) = validate_of(parse_quote! {
            #[from(std::io::Error(missing: 1), std::fmt::Error)]
            struct T { kind: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("unknown field `missing`"), "{faults}");
        assert_eq!(model.expect("a model").conversions.len(), 1);
    }
}
