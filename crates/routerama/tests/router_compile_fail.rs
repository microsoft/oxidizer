// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time diagnostics for the generated HTTP handler contract.
//!
//! Feature-gated diagnostics live in `feature_gates.rs` because the workspace
//! test command enables every feature, which would silence them.
//!
//! Every case here pins a diagnostic whose primary span is a single token.
//! `syn` joins multi-token spans only when the compiler running the proc macro
//! offers `proc_macro::Span::join`, so a diagnostic pointing at a whole type,
//! parameter, or generics list renders one caret on stable and a full
//! underline on nightly. Those diagnostics are asserted by message in
//! `routerama_build`'s expansion unit tests instead, which are identical on
//! every toolchain.

use trybuild::TestCases;

#[test]
#[cfg_attr(miri, ignore)]
fn invalid_handler_contracts_are_rejected() {
    let tests = TestCases::new();
    tests.compile_fail("tests/ui/handler_not_async.rs");
    tests.compile_fail("tests/ui/handler_missing_self.rs");
    tests.compile_fail("tests/ui/handler_missing_return_type.rs");
    tests.compile_fail("tests/ui/router_without_routes.rs");
    tests.compile_fail("tests/ui/duplicate_body_markers.rs");
    tests.compile_fail("tests/ui/missing_state_projection.rs");
    tests.compile_fail("tests/ui/incompatible_extractor_state.rs");
    tests.compile_fail("tests/ui/incompatible_body_extractor_state.rs");
    tests.compile_fail("tests/ui/malformed_router_arguments.rs");
    tests.compile_fail("tests/ui/erased_mounts_without_state.rs");
    tests.compile_fail("tests/ui/wrong_fixed_state_call.rs");
    tests.compile_fail("tests/ui/named_parts_lifetime.rs");
    tests.compile_fail("tests/ui/different_alias_predicates.rs");
    tests.compile_fail("tests/ui/duplicate_route_alias.rs");
    tests.compile_fail("tests/ui/malformed_priority.rs");
    tests.compile_fail("tests/ui/duplicate_priority.rs");
    tests.compile_fail("tests/ui/overlap_missing_priority.rs");
    tests.compile_fail("tests/ui/overlap_duplicate_priority.rs");
    tests.compile_fail("tests/ui/overlap_capture_names.rs");
    tests.compile_fail("tests/ui/overlap_capture_types.rs");
    tests.compile_fail("tests/ui/predicate_free_overlap_priority.rs");
    tests.compile_fail("tests/ui/identical_overlap_predicates.rs");
    tests.compile_fail("tests/ui/dynamic_priority.rs");
    tests.compile_fail("tests/ui/duplicate_fallback.rs");
    tests.compile_fail("tests/ui/duplicate_catcher.rs");
    tests.compile_fail("tests/ui/unused_catcher.rs");
    tests.compile_fail("tests/ui/invalid_fallback_signature.rs");
    tests.compile_fail("tests/ui/invalid_catcher_signature.rs");
    tests.compile_fail("tests/ui/mismatched_catcher_type.rs");
    tests.compile_fail("tests/ui/borrowed_catcher_type.rs");
    tests.compile_fail("tests/ui/generic_catcher.rs");
    tests.compile_fail("tests/ui/interceptor_duplicate_marker.rs");
    tests.compile_fail("tests/ui/interceptor_on_policy.rs");
    tests.compile_fail("tests/ui/before_bad_return.rs");
    tests.compile_fail("tests/ui/after_bad_return.rs");
    tests.compile_fail("tests/ui/before_wrong_context.rs");
    tests.compile_fail("tests/ui/router_wide_before_wrong_context.rs");
    tests.compile_fail("tests/ui/transform_missing_limit.rs");
    tests.compile_fail("tests/ui/transform_no_handler.rs");
    tests.compile_fail("tests/ui/transform_consumer_conflict.rs");
    tests.compile_fail("tests/ui/transform_wrong_buffer_type.rs");
    tests.compile_fail("tests/ui/transform_buffered_generic.rs");
    tests.compile_fail("tests/ui/transform_stream_not_generic.rs");
    tests.compile_fail("tests/ui/transform_stream_body_mismatch.rs");
    tests.compile_fail("tests/ui/transform_stream_with_limit.rs");
    tests.compile_fail("tests/ui/transform_stream_generic_response.rs");
    tests.compile_fail("tests/ui/transform_stream_consumer_conflict.rs");
    tests.compile_fail("tests/ui/before_unknown_handler.rs");
    tests.compile_fail("tests/ui/interceptor_on_route.rs");
    tests.compile_fail("tests/ui/two_transforms_one_handler.rs");
    tests.compile_fail("tests/ui/transform_bad_return.rs");
    #[cfg(feature = "form")]
    tests.compile_fail("tests/ui/borrowed_form.rs");
    #[cfg(feature = "resolve")]
    tests.compile_fail("tests/ui/resolver_request_predicate.rs");
    #[cfg(feature = "resolve")]
    tests.compile_fail("tests/ui/resolver_priority.rs");
    #[cfg(feature = "tower")]
    {
        tests.compile_fail("tests/ui/tower_non_send_state.rs");
        tests.compile_fail("tests/ui/generated_tower_non_send_body.rs");
    }
}
