// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-fail tests pinning the diagnostics `#[display(...)]` produces.
//!
//! These go through the real `#[ohno::error]` and `#[derive(Error)]` entry points, so they cover
//! the handoff between the two, and the `.stderr` snapshots pin *where* each diagnostic points —
//! neither of which the unit tests in `ohno_macros` can observe.

#[test]
#[cfg_attr(miri, ignore)]
fn display_diagnostics() {
    let t = trybuild::TestCases::new();

    // Arguments are implicitly scoped to `self`, so the `self.` prefix must be omitted.
    t.compile_fail("tests/ui/display_self_prefixed_argument.rs");

    // An unknown field is reported by the macro, listing the fields the user declared.
    t.compile_fail("tests/ui/display_unknown_field.rs");

    // A root that cannot legally follow `self.` is reported by the macro, rather than expanding
    // to code that does not parse.
    t.compile_fail("tests/ui/display_unsupported_argument_root.rs");

    // `#[error]` takes no arguments, so an unrecognized one is reported rather than ignored.
    t.compile_fail("tests/ui/error_attribute_arguments.rs");

    // Only one field can hold the OhnoCore, and the marker belongs on that field alone.
    t.compile_fail("tests/ui/error_attribute_on_several_fields.rs");

    // `#[ohno::error]` generates the error representation from the field it injects, so no other
    // field may be marked.
    t.compile_fail("tests/ui/ohno_error_marked_field.rs");

    // The marker on the field `#[ohno::error]` adds is reserved, so a field already carrying it
    // was written by hand.
    t.compile_fail("tests/ui/ohno_error_reserved_marker.rs");

    // `#[ohno::error]` adds the OhnoCore field, so a hand-written constructor would have to name
    // a field the attribute chose; opting out of the generated ones is rejected.
    t.compile_fail("tests/ui/ohno_error_no_constructors.rs");

    // A tuple index reaching the `OhnoCore` appended by `#[ohno::error]` is unknown, while the
    // index of the declared field in the same fixture resolves.
    t.compile_fail("tests/ui/display_tuple_index_reaching_injected_core.rs");

    // A raw identifier is echoed back with its prefix, which is the spelling the user must write.
    t.compile_fail("tests/ui/display_raw_identifier_unknown_field.rs");

    // An argument's root is found through every expression form that keeps a term leftmost, and
    // reported at that term rather than at the whole expression.
    t.compile_fail("tests/ui/display_binary_argument_root.rs");

    // An unbalanced brace is reported rather than parsed into a different, valid template.
    t.compile_fail("tests/ui/display_unbalanced_brace.rs");
}
