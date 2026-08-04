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

    // Only one field can hold the OhnoCore, and the generated marker only ever appears on one.
    t.compile_fail("tests/ui/error_attribute_on_several_fields.rs");
    t.compile_fail("tests/ui/error_attribute_on_another_type.rs");

    // A tuple index reaching the `OhnoCore` appended by `#[ohno::error]` is unknown, while the
    // index of the declared field in the same fixture resolves.
    t.compile_fail("tests/ui/display_tuple_index_reaching_injected_core.rs");
}
