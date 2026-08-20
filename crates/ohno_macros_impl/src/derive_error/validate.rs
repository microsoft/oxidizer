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

    let shape = Shape::new(fields, core?, ast.style);

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
///
/// The generated marker occupies the core slot before any of these are read, so when it is present
/// every hand-written `#[error]` is one marker too many, wherever in the struct it sits.
fn report_duplicate_markers(fields: &[AstField], errors: &mut Errors) {
    let mut seen_marked_field = fields.iter().any(|field| field.generated);

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
}
