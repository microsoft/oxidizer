// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reachability suite: everything here drives the crate's real public API.
//!
//! `derive_error`, `enrich_err` and `error` are the whole public surface, and they take exactly
//! what the proc-macro shim hands them. Any behaviour that cannot be provoked from here cannot be
//! provoked by a user of `ohno` either.

use ohno_macros_impl::{derive_error, enrich_err, error};
use proc_macro2::TokenStream;
use quote::quote;

/// The derive's expansion, as text.
fn derived(input: TokenStream) -> String {
    derive_error(input).to_string()
}

/// The diagnostics the derive reports, as text. Panics when it reported none.
fn derive_faults(input: TokenStream) -> String {
    let expanded = derived(input);
    assert!(expanded.contains("compile_error"), "expected a diagnostic, got: {expanded}");
    expanded
}

/// The `#[ohno::error]` expansion, as text.
fn attributed(item: TokenStream) -> String {
    error(TokenStream::new(), item).to_string()
}

/// The diagnostics `#[ohno::error]` reports, as text. Panics when it reported none.
fn attribute_faults(item: TokenStream) -> String {
    let expanded = attributed(item);
    assert!(expanded.contains("compile_error"), "expected a diagnostic, got: {expanded}");
    expanded
}

/// The `#[enrich_err(...)]` expansion, as text.
fn enriched(args: TokenStream, item: TokenStream) -> String {
    enrich_err(args, item).to_string()
}

/// The diagnostics `#[enrich_err(...)]` reports, as text. Panics when it reported none.
fn enrich_faults(args: TokenStream, item: TokenStream) -> String {
    let expanded = enriched(args, item);
    assert!(expanded.contains("compile_error"), "expected a diagnostic, got: {expanded}");
    expanded
}

// ---------------------------------------------------------------------------
// derive: shapes
// ---------------------------------------------------------------------------

#[test]
fn a_named_struct_generates_every_item() {
    let expanded = derived(quote! {
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("Display"), "{expanded}");
    assert!(expanded.contains("Debug"), "{expanded}");
    assert!(expanded.contains("fn new"), "{expanded}");
    assert!(expanded.contains("fn caused_by"), "{expanded}");
}

#[test]
fn a_tuple_struct_generates_positional_items() {
    let expanded = derived(quote! {
        struct T(String, ohno::OhnoCore);
    });
    assert!(expanded.contains("fn new"), "{expanded}");
}

#[test]
fn a_single_field_struct_takes_no_constructor_parameters() {
    let expanded = derived(quote! {
        struct T { inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("fn new"), "{expanded}");
}

#[test]
fn a_core_in_the_middle_keeps_declaration_order() {
    let expanded = derived(quote! {
        struct T { first: String, inner: ohno::OhnoCore, last: u32 }
    });
    assert!(expanded.contains("first"), "{expanded}");
    assert!(expanded.contains("last"), "{expanded}");
}

#[test]
fn the_core_may_be_marked_rather_than_named() {
    let expanded = derived(quote! {
        struct T { path: String, #[error] mine: Renamed }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_unit_struct_has_no_core() {
    let _ = derive_faults(quote!(
        struct T;
    ));
}

#[test]
fn an_enum_is_rejected() {
    let faults = derive_faults(quote! {
        enum T { A, B }
    });
    assert!(faults.contains("struct"), "{faults}");
}

#[test]
fn a_union_is_rejected() {
    let _ = derive_faults(quote! {
        union T { a: u32 }
    });
}

#[test]
fn a_struct_without_a_core_is_rejected() {
    let _ = derive_faults(quote! {
        struct T { path: String }
    });
}

#[test]
fn two_marked_cores_are_rejected() {
    let _ = derive_faults(quote! {
        struct T { #[error] a: ohno::OhnoCore, #[error] b: ohno::OhnoCore }
    });
}

#[test]
fn two_unmarked_cores_are_rejected() {
    let _ = derive_faults(quote! {
        struct T { a: ohno::OhnoCore, b: ohno::OhnoCore }
    });
}

// ---------------------------------------------------------------------------
// derive: generics
// ---------------------------------------------------------------------------

#[test]
fn generics_thread_through_every_impl() {
    let expanded = derived(quote! {
        struct T<A, B> { a: A, b: B, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("Display"), "{expanded}");
}

#[test]
fn a_lifetime_threads_through() {
    let expanded = derived(quote! {
        struct T<'a> { path: &'a str, inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_where_clause_survives() {
    let expanded = derived(quote! {
        struct T<A> where A: Clone { a: A, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("where"), "{expanded}");
}

// ---------------------------------------------------------------------------
// derive: suppressing flags
// ---------------------------------------------------------------------------

#[test]
fn no_debug_removes_the_debug_impl() {
    let expanded = derived(quote! {
        #[no_debug]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("Debug"), "{expanded}");
}

#[test]
fn no_constructors_removes_the_constructors() {
    let expanded = derived(quote! {
        #[no_constructors]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("fn new"), "{expanded}");
}

#[test]
fn a_suppressing_flag_takes_no_arguments() {
    let _ = derive_faults(quote! {
        #[no_debug(yes)]
        struct T { inner: ohno::OhnoCore }
    });
    let _ = derive_faults(quote! {
        #[no_constructors = 1]
        struct T { inner: ohno::OhnoCore }
    });
}

// ---------------------------------------------------------------------------
// derive: `#[display(...)]` templates
// ---------------------------------------------------------------------------

#[test]
fn a_static_template_lowers_to_a_literal() {
    let expanded = derived(quote! {
        #[display("nothing interpolated")]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("format !"), "{expanded}");
}

#[test]
fn escapes_are_resolved_for_a_literal_message() {
    let expanded = derived(quote! {
        #[display("braces {{ and }}")]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_named_placeholder_becomes_a_field_access() {
    let expanded = derived(quote! {
        #[display("failed for {path}")]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("self . path"), "{expanded}");
}

#[test]
fn a_tuple_field_is_named_by_index() {
    let expanded = derived(quote! {
        #[display("failed for {0}")]
        struct T(String, ohno::OhnoCore);
    });
    assert!(expanded.contains("self . 0"), "{expanded}");
}

#[test]
fn a_raw_identifier_is_referenced_with_its_prefix() {
    let expanded = derived(quote! {
        #[display("failed for {r#type}")]
        struct T { r#type: String, inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_format_spec_survives_lowering() {
    let expanded = derived(quote! {
        #[display("padded {path:>8}")]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains(":>8"), "{expanded}");
}

#[test]
fn an_unknown_placeholder_lists_the_available_fields() {
    let faults = derive_faults(quote! {
        #[display("failed for {missing}")]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("available fields"), "{faults}");
}

#[test]
fn nothing_is_referenceable_when_every_field_is_generated() {
    // The core `#[ohno::error]` adds carries the reserved marker, which makes it unreferenceable.
    // This is the shape `#[ohno::error] struct T;` produces.
    let faults = derive_faults(quote! {
        #[display("failed for {missing}")]
        struct T {
            #[doc = " ohno::generated-core@7f3d9c2a"]
            inner: ohno::OhnoCore
        }
    });
    assert!(faults.contains("no fields"), "{faults}");
}

#[test]
fn a_stray_closing_brace_stops_the_lowering() {
    let _ = derive_faults(quote! {
        #[display("stray } brace")]
        struct T { inner: ohno::OhnoCore }
    });
}

#[test]
fn an_unbalanced_brace_stops_the_lowering() {
    let _ = derive_faults(quote! {
        #[display("unbalanced { brace")]
        struct T { inner: ohno::OhnoCore }
    });
}

#[test]
fn only_one_display_attribute_is_accepted() {
    let faults = derive_faults(quote! {
        #[display("first")]
        #[display("second")]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(faults.contains("only one"), "{faults}");
}

#[test]
fn a_display_attribute_needs_a_literal_template() {
    let _ = derive_faults(quote! {
        #[display(not_a_literal)]
        struct T { inner: ohno::OhnoCore }
    });
}

// ---------------------------------------------------------------------------
// derive: `#[display(...)]` positional arguments
// ---------------------------------------------------------------------------

#[test]
fn a_positional_argument_is_scoped_and_parenthesized() {
    let expanded = derived(quote! {
        #[display("failed for {}", path)]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("& (self . path)"), "{expanded}");
}

#[test]
fn every_leftmost_argument_form_is_scoped() {
    for argument in [
        quote!(path.len()),
        quote!(path.inner),
        quote!(path[0]),
        quote!(path as u64),
        quote!(path?),
        quote!(path..10),
        quote!(path.0.1),
    ] {
        let expanded = derived(quote! {
            #[display("value {}", #argument)]
            struct T { path: String, inner: ohno::OhnoCore }
        });
        assert!(expanded.contains("self ."), "{argument}: {expanded}");
    }
}

#[test]
fn a_numeric_argument_roots_at_a_tuple_field() {
    // An integer literal names a tuple field; a float is a nested tuple access whose leading
    // component names the field.
    for argument in [quote!(0), quote!(0.1), quote!(0.abs())] {
        let expanded = derived(quote! {
            #[display("value {}", #argument)]
            struct T(String, ohno::OhnoCore);
        });
        assert!(expanded.contains("self ."), "{argument}: {expanded}");
    }
}

#[test]
fn an_argument_may_call_a_method_of_self() {
    let expanded = derived(quote! {
        #[display("value {}", describe())]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_self_prefixed_argument_is_reported() {
    let faults = derive_faults(quote! {
        #[display("value {}", self.path)]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("without a `self.` prefix"), "{faults}");
}

#[test]
fn an_unknown_argument_root_is_reported() {
    let faults = derive_faults(quote! {
        #[display("value {}", missing.len())]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("unknown field"), "{faults}");
}

#[test]
fn an_unsupported_argument_root_is_reported() {
    for argument in [
        quote!(Self::LABEL.len()),
        quote!("prefix".len()),
        quote!(std::mem::size_of::<u8>()),
        quote!(<T>::VALUE),
        quote!(<T as Trait>::VALUE),
        quote!((path)),
        quote!(-path),
        quote!(..10),
        quote!('c'),
        quote!(0u8),
        quote!(1usize.abs()),
        quote!(0.1f32),
        quote!(2.0f64),
    ] {
        let faults = derive_faults(quote! {
            #[display("value {}", #argument)]
            struct T { path: String, inner: ohno::OhnoCore }
        });
        assert!(faults.contains("rooted in a field or method"), "{argument}: {faults}");
    }
}

#[test]
fn an_unconsumed_argument_is_reported() {
    let faults = derive_faults(quote! {
        #[display("no placeholder", path)]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("not consumed"), "{faults}");
}

#[test]
fn too_few_arguments_are_reported() {
    let faults = derive_faults(quote! {
        #[display("{} and {}", path)]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("more `{}` placeholders"), "{faults}");
}

#[test]
fn every_fault_in_one_template_is_reported_together() {
    let faults = derive_faults(quote! {
        #[display("{missing} {} {}", self.path)]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("unknown field"), "{faults}");
    assert!(faults.contains("without a `self.` prefix"), "{faults}");
}

#[test]
fn a_trailing_comma_after_the_arguments_is_accepted() {
    let expanded = derived(quote! {
        #[display("value {}", path,)]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

// ---------------------------------------------------------------------------
// derive: `#[from(...)]`
// ---------------------------------------------------------------------------

#[test]
fn a_from_attribute_generates_a_conversion() {
    let expanded = derived(quote! {
        #[from(std::io::Error)]
        struct T { source: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("From"), "{expanded}");
}

#[test]
fn several_types_in_one_from_attribute_each_convert() {
    let expanded = derived(quote! {
        #[from(std::io::Error, std::fmt::Error)]
        struct T { source: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("From"), "{expanded}");
}

#[test]
fn a_generic_source_type_keeps_its_arguments() {
    let expanded = derived(quote! {
        #[from(Wrapper<u8, u16>)]
        struct T { source: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("Wrapper"), "{expanded}");
}

#[test]
fn conversions_initialize_every_non_core_field() {
    let expanded = derived(quote! {
        #[from(std::io::Error(path: "unknown".to_owned()))]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("unknown"), "{expanded}");
}

#[test]
fn a_from_override_may_name_a_tuple_index() {
    let expanded = derived(quote! {
        #[from(std::io::Error(0: "unknown".to_owned()))]
        struct T(String, ohno::OhnoCore);
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_from_attribute_needs_a_parenthesized_list() {
    let faults = derive_faults(quote! {
        #[from = 1]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(faults.contains("parenthesized list"), "{faults}");
}

#[test]
fn an_empty_from_attribute_is_rejected() {
    let faults = derive_faults(quote! {
        #[from()]
        struct T { inner: ohno::OhnoCore }
    });
    assert!(faults.contains("at least one type"), "{faults}");
}

#[test]
fn a_from_entry_must_start_with_a_type() {
    let _ = derive_faults(quote! {
        #[from((path: 1))]
        struct T { path: String, inner: ohno::OhnoCore }
    });
}

#[test]
fn a_from_override_naming_an_unknown_field_is_rejected() {
    let _ = derive_faults(quote! {
        #[from(std::io::Error(missing: 1))]
        struct T { path: String, inner: ohno::OhnoCore }
    });
}

#[test]
fn a_from_conversion_defaults_every_other_field() {
    // Fields the override list does not name are `Default::default()`, so a multi-field struct
    // needs no override to convert.
    let expanded = derived(quote! {
        #[from(std::io::Error)]
        struct T { path: String, other: u32, inner: ohno::OhnoCore }
    });
    assert!(expanded.contains("Default :: default"), "{expanded}");
}

// ---------------------------------------------------------------------------
// `#[ohno::error]`
// ---------------------------------------------------------------------------

#[test]
fn the_attribute_adds_a_named_core() {
    let expanded = attributed(quote! {
        struct T { path: String }
    });
    assert!(expanded.contains("ohno_core"), "{expanded}");
}

#[test]
fn a_colliding_core_name_is_numbered() {
    let expanded = attributed(quote! {
        struct T { ohno_core: u32, ohno_core_1: u32 }
    });
    assert!(expanded.contains("ohno_core_2"), "{expanded}");
}

#[test]
fn a_tuple_struct_gains_a_trailing_core() {
    let expanded = attributed(quote!(
        struct T(String);
    ));
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_unit_struct_becomes_a_tuple_struct() {
    let expanded = attributed(quote!(
        struct T;
    ));
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn other_attributes_and_docs_survive() {
    let expanded = attributed(quote! {
        /// Documentation for T.
        #[derive(Clone)]
        #[display("failed for {path}")]
        struct T { path: String }
    });
    assert!(expanded.contains("Clone"), "{expanded}");
    assert!(expanded.contains("Documentation for T"), "{expanded}");
}

#[test]
fn an_ordinary_doc_comment_on_a_field_is_left_alone() {
    let expanded = attributed(quote! {
        struct T {
            /// Where the failure happened.
            path: String,
        }
    });
    assert!(expanded.contains("Where the failure happened"), "{expanded}");
}

#[test]
fn a_marked_field_is_rejected_by_the_attribute() {
    let _ = attribute_faults(quote! {
        struct T { path: String, #[error] mine: ohno::OhnoCore }
    });
}

#[test]
fn a_hand_written_reserved_marker_is_rejected() {
    let _ = attribute_faults(quote! {
        struct T {
            path: String,
            #[doc = " ohno::generated-core@7f3d9c2a"]
            mine: ohno::OhnoCore,
        }
    });
}

#[test]
fn the_attribute_rejects_no_constructors() {
    let _ = attribute_faults(quote! {
        #[no_constructors]
        struct T { path: String }
    });
}

#[test]
fn the_attribute_rejects_a_non_struct() {
    let _ = attribute_faults(quote!(
        enum T {
            A,
        }
    ));
}

#[test]
fn a_rejected_struct_is_not_rewritten() {
    let expanded = attribute_faults(quote! {
        struct T { path: String, #[error] mine: ohno::OhnoCore }
    });
    assert!(!expanded.contains("struct T"), "{expanded}");
}

#[test]
fn the_attribute_takes_no_arguments() {
    let expanded = error(
        quote!(anything),
        quote! {
            struct T { path: String }
        },
    )
    .to_string();
    assert!(expanded.contains("takes no arguments"), "{expanded}");
}

#[test]
fn the_attribute_reports_an_unparsable_item() {
    let expanded = error(TokenStream::new(), quote!(1 + 1)).to_string();
    assert!(expanded.contains("compile_error"), "{expanded}");
}

// ---------------------------------------------------------------------------
// `#[enrich_err(...)]`
// ---------------------------------------------------------------------------

#[test]
fn a_bare_attribute_names_the_function() {
    let expanded = enriched(
        TokenStream::new(),
        quote! {
            fn load() -> Result<(), MyError> { inner() }
        },
    );
    assert!(expanded.contains("load"), "{expanded}");
    assert!(expanded.contains("file !"), "{expanded}");
    assert!(expanded.contains("line !"), "{expanded}");
}

#[test]
fn a_literal_message_renders_without_format() {
    let expanded = enriched(
        quote!("could not load"),
        quote! {
            fn load() -> Result<(), MyError> { inner() }
        },
    );
    assert!(!expanded.contains("format !"), "{expanded}");
}

#[test]
fn an_inline_capture_goes_through_format() {
    let expanded = enriched(
        quote!("could not load {path}"),
        quote! {
            fn load(path: &str) -> Result<(), MyError> { inner() }
        },
    );
    assert!(expanded.contains("format !"), "{expanded}");
}

#[test]
fn arguments_are_passed_through_unchanged() {
    let expanded = enriched(
        quote!("could not load {}", path),
        quote! {
            fn load(path: &str) -> Result<(), MyError> { inner() }
        },
    );
    assert!(expanded.contains("format !"), "{expanded}");
    assert!(expanded.contains("path"), "{expanded}");
}

#[test]
fn a_self_prefixed_argument_is_left_alone() {
    let expanded = enriched(
        quote!("could not load {}", self.path),
        quote! {
            fn load(&self) -> Result<(), MyError> { inner() }
        },
    );
    assert!(expanded.contains("self . path"), "{expanded}");
}

#[test]
fn the_signature_survives_untouched() {
    let expanded = enriched(
        TokenStream::new(),
        quote! {
            pub(crate) fn load<A: Clone>(path: &str, count: usize) -> Result<A, MyError> where A: Send { inner() }
        },
    );
    assert!(expanded.contains("pub (crate)"), "{expanded}");
    assert!(expanded.contains("where"), "{expanded}");
}

#[test]
fn the_body_runs_inside_a_closure() {
    let expanded = enriched(
        TokenStream::new(),
        quote! {
            fn load() -> Result<(), MyError> { inner() }
        },
    );
    assert!(expanded.contains("| |"), "{expanded}");
}

#[test]
fn an_async_function_awaits_an_async_block() {
    let expanded = enriched(
        TokenStream::new(),
        quote! {
            async fn load() -> Result<(), MyError> { inner().await }
        },
    );
    assert!(expanded.contains("async"), "{expanded}");
    assert!(expanded.contains("await"), "{expanded}");
}

#[test]
fn a_non_function_is_rejected() {
    let faults = enrich_faults(
        TokenStream::new(),
        quote! {
            struct T;
        },
    );
    assert!(faults.contains("functions only"), "{faults}");
}

#[test]
fn a_missing_return_type_is_rejected() {
    let faults = enrich_faults(
        TokenStream::new(),
        quote! {
            fn load() { inner(); }
        },
    );
    assert!(faults.contains("needs a return type"), "{faults}");
}

#[test]
fn a_non_literal_first_argument_is_rejected() {
    let _ = enrich_faults(
        quote!(not_a_literal),
        quote! {
            fn load() -> Result<(), MyError> { inner() }
        },
    );
}

#[test]
fn an_unparsable_item_is_rejected() {
    let expanded = enriched(TokenStream::new(), quote!(1 + 1));
    assert!(expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn an_unparsable_derive_input_is_rejected() {
    let expanded = derived(quote!(
        fn not_a_type() {}
    ));
    assert!(expanded.contains("compile_error"), "{expanded}");
}

// ---------------------------------------------------------------------------
// Reachability follow-ups: paths the first pass of this suite missed.
// ---------------------------------------------------------------------------

#[test]
fn a_binary_or_await_argument_roots_at_its_leftmost_term() {
    for argument in [quote!(path * 2), quote!(path.await), quote!(path + other)] {
        let expanded = derived(quote! {
            #[display("value {}", #argument)]
            struct T { path: String, other: u32, inner: ohno::OhnoCore }
        });
        assert!(expanded.contains("self ."), "{argument}: {expanded}");
    }
}

#[test]
fn a_from_key_on_a_tuple_struct_must_be_an_index() {
    let faults = derive_faults(quote! {
        #[from(std::io::Error(path: 1))]
        struct T(String, ohno::OhnoCore);
    });
    assert!(faults.contains("field indexes, not names"), "{faults}");
}

#[test]
fn a_from_key_naming_the_core_is_rejected() {
    let faults = derive_faults(quote! {
        #[from(std::io::Error(inner: 1))]
        struct T { path: String, inner: ohno::OhnoCore }
    });
    assert!(faults.contains("holds the OhnoCore"), "{faults}");
}

#[test]
fn a_field_marked_twice_reports_the_repeat() {
    let _ = derive_faults(quote! {
        struct T { #[error] #[error] inner: ohno::OhnoCore }
    });
}

#[test]
fn a_non_doc_field_attribute_is_not_the_reserved_marker() {
    // `is_generated_marker` inspects every field attribute. These are the shapes that look like
    // the marker without being it, and each takes a different early exit.
    let expanded = attributed(quote! {
        struct T {
            #[other = " ohno::generated-core@7f3d9c2a"]
            path: String,
        }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");

    let expanded = attributed(quote! {
        struct T {
            #[doc = 1]
            path: String,
        }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");

    let expanded = attributed(quote! {
        struct T {
            #[doc = concat!(" ohno::generated-core@7f3d9c2a")]
            path: String,
        }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_doc_comment_that_merely_mentions_the_marker_is_not_the_marker() {
    let expanded = attributed(quote! {
        struct T {
            /// The ohno generated core field is not this one.
            path: String,
        }
    });
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn a_from_entry_opening_with_an_indexed_override_list_is_rejected() {
    // `holds_overrides` accepts an ident OR a literal as the key, so a tuple-index override list
    // in leading position has to be rejected the same way a named one is.
    let _ = derive_faults(quote! {
        #[from((0: 1))]
        struct T(String, ohno::OhnoCore);
    });
}

#[test]
fn arguments_force_a_format_call_even_without_placeholders() {
    // `Message::opaque` renders a literal only when there are no arguments AND no braces; this is
    // the arguments-without-braces half of that condition.
    let expanded = enriched(
        quote!("plain message", path),
        quote! {
            fn load(path: &str) -> Result<(), MyError> { inner() }
        },
    );
    assert!(expanded.contains("format !"), "{expanded}");
}
