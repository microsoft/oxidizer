#![allow(missing_docs)] // Test helpers.

/// Verifies that proc-macro validation and end-to-end resolution errors produce the expected compiler diagnostics.
#[test]
#[cfg_attr(miri, ignore)]
fn compile_fail() {
    let t = trybuild::TestCases::new();

    // Proc macro validation errors
    t.compile_fail("tests/compile_fail/resolvable_on_trait_impl.rs");
    t.compile_fail("tests/compile_fail/resolvable_missing_new.rs");
    t.compile_fail("tests/compile_fail/resolvable_non_ref_param.rs");
    t.compile_fail("tests/compile_fail/resolvable_mut_ref_param.rs");
    t.compile_fail("tests/compile_fail/resolvable_wrong_return.rs");
    t.compile_fail("tests/compile_fail/resolvable_no_return.rs");
    t.compile_fail("tests/compile_fail/resolvable_generic_impl.rs");
    t.compile_fail("tests/compile_fail/resolvable_self_receiver.rs");

    // End-to-end resolution errors
    t.compile_fail("tests/compile_fail/missing_dependency.rs");
    t.compile_fail("tests/compile_fail/dependency_cycle.rs");
    t.compile_fail("tests/compile_fail/scoped_type_not_resolvable_from_parent.rs");
    t.compile_fail("tests/compile_fail/scoped_type_not_resolvable_from_parent_cv.rs");

    // Base macro validation errors
    t.compile_fail("tests/compile_fail/scoped_parent_not_module_qualified.rs");
    t.compile_fail("tests/compile_fail/scoped_base_as_top_level.rs");

    // Override builder validation errors
    t.compile_fail("tests/compile_fail/provide_chain_not_dependency.rs");
}
